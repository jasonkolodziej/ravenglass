//! [`ImapBackend`]: the `MailBackend` for one IMAP + SMTP account.
//!
//! The engine polls backend futures from a plain thread (`ravenglass_engine::runner::block_on`),
//! not from a tokio runtime, and tokio's sockets and timers only work inside one. So every call
//! is spawned onto a runtime the backend owns (or one it was given), and the future handed back
//! merely waits for that task: it can be polled from anywhere, and dropping it cancels the task.

use crate::config::{Credential, ImapConfig};
use crate::conn::{FETCH_FULL, FETCH_NO_STRUCTURE, FETCH_WHOLE_TEXT, Inner, with_conn};
use crate::net::UNREADABLE;
use crate::{idle, ops, smtp};
use mail_core::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::task::{Context, Poll};
use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;

pub struct ImapBackend {
    inner: Arc<Inner>,
    handle: Handle,
    /// The runtime, when this backend started it.
    owned: Option<Runtime>,
}

impl ImapBackend {
    /// A backend with a small runtime of its own (two worker threads: commands and IDLE).
    /// Nothing connects until the first call.
    pub fn new(config: ImapConfig) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("ravenglass-imap").enable_all().build()
            .map_err(|_| MailError::Server("could not start the IMAP worker threads".into()))?;
        Ok(Self { inner: Arc::new(Inner::new(config)?), handle: runtime.handle().clone(), owned: Some(runtime) })
    }

    /// A backend that runs on a tokio runtime the host already has (the Relay).
    pub fn with_handle(config: ImapConfig, handle: Handle) -> Result<Self> {
        Ok(Self { inner: Arc::new(Inner::new(config)?), handle, owned: None })
    }

    /// Replace the credential, e.g. with a refreshed OAuth token. Used from the next connect on;
    /// an open connection stays authenticated as it is.
    pub fn set_credential(&self, credential: Credential) { *self.inner.credential.lock().unwrap() = credential; }

    /// Run `work` on the runtime and return a future that waits for it from any thread.
    fn spawn<T: Send + 'static>(&self, work: impl Future<Output = Result<T>> + Send + 'static) -> BoxFut<'static, Result<T>> {
        Box::pin(Bridged(self.handle.spawn(work)))
    }
}

impl Drop for ImapBackend {
    fn drop(&mut self) {
        // A plain drop would block on running tasks, and panics inside another async runtime.
        if let Some(runtime) = self.owned.take() { runtime.shutdown_background(); }
    }
}

/// Waits for a spawned task; cancels it if nobody is waiting any more.
pub(crate) struct Bridged<T>(pub(crate) JoinHandle<Result<T>>);

impl<T> Future for Bridged<T> {
    type Output = Result<T>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(|joined| joined.unwrap_or_else(|_| Err(MailError::Server("the mail task stopped unexpectedly".into()))))
    }
}

impl<T> Drop for Bridged<T> {
    fn drop(&mut self) { self.0.abort(); }
}

impl MailBackend for ImapBackend {
    fn account(&self) -> &AccountId { &self.inner.config.account }

    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> {
        let inner = self.inner.clone();
        self.spawn(async move { with_conn!(inner, |conn| ops::list_folders(&inner, conn)) })
    }

    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> {
        let (inner, folder, since) = (self.inner.clone(), folder.clone(), since.clone());
        self.spawn(async move {
            loop {
                let synced: Result<FolderDelta> = with_conn!(inner, |conn| ops::sync_folder(&inner, conn, &folder, &since));
                // The server answered in a form the parser rejects. Ask for less and try again;
                // the level sticks, so later syncs go straight to what works.
                let unreadable = matches!(&synced, Err(MailError::Server(text)) if text == UNREADABLE);
                let level = inner.fetch_level.load(Ordering::Relaxed);
                if !unreadable || level >= FETCH_NO_STRUCTURE { return synced; }
                inner.fetch_level.store(if level == FETCH_FULL { FETCH_WHOLE_TEXT } else { FETCH_NO_STRUCTURE }, Ordering::Relaxed);
            }
        })
    }

    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> {
        let (inner, msg) = (self.inner.clone(), msg.clone());
        self.spawn(async move { with_conn!(inner, |conn| ops::fetch_body(&inner, conn, &msg)) })
    }

    fn set_flags<'a>(&'a self, msgs: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>> {
        let (inner, msgs) = (self.inner.clone(), msgs.to_vec());
        self.spawn(async move { with_conn!(inner, |conn| ops::set_flags(&inner, conn, &msgs, add, remove)) })
    }

    fn move_to<'a>(&'a self, msgs: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>> {
        let (inner, msgs, to) = (self.inner.clone(), msgs.to_vec(), to.clone());
        self.spawn(async move { with_conn!(inner, |conn| ops::move_to(&inner, conn, &msgs, &to)) })
    }

    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> {
        let (inner, raw) = (self.inner.clone(), raw.to_vec());
        self.spawn(async move { with_conn!(inner, |conn| ops::append_draft(&inner, conn, &raw)) })
    }

    fn send<'a>(&'a self, raw: &'a [u8], from: &'a Address, rcpt: &'a [Address]) -> BoxFut<'a, Result<()>> {
        let (inner, raw, from) = (self.inner.clone(), raw.to_vec(), from.email.clone());
        let rcpt: Vec<String> = rcpt.iter().map(|r| r.email.clone()).collect();
        self.spawn(async move {
            let credential = inner.credential.lock().unwrap().clone();
            let config = &inner.config;
            let submission = smtp::Submission { endpoint: &config.smtp, tls: &inner.tls, username: &config.username, credential: &credential, timeout: config.timeout };
            smtp::send(submission, &raw, &from, &rcpt).await?;
            if config.save_sent {
                // The message has gone out. Failing here would make the outbox send it again, so a
                // copy that cannot be saved is given up on; the recipients still have the mail.
                let saved: Result<()> = with_conn!(inner, |conn| ops::append_sent(&inner, conn, &raw));
                let _ = saved;
            }
            Ok(())
        })
    }

    fn idle<'a>(&'a self, folder: &'a FolderId, events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> {
        self.spawn(idle::run(self.inner.clone(), folder.clone(), events))
    }
}

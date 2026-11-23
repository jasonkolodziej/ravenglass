//! The one interface every mail source implements (IMAP now; JMAP / Gmail API later; mock in tests).

use crate::types::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc::Sender;

/// A boxed, sendable future — keeps the trait dyn-compatible without an `async-trait` dependency.
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait MailBackend: Send + Sync {
    fn account(&self) -> &AccountId;
    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>>;
    /// Changes since `since` (an empty token means a full sync).
    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>>;
    /// Lazy, on open.
    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>>;
    fn set_flags<'a>(&'a self, msgs: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>>;
    /// MOVE, else COPY + EXPUNGE.
    fn move_to<'a>(&'a self, msgs: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>>;
    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>>;
    /// SMTP submit, then APPEND to Sent unless the server does it itself.
    fn send<'a>(&'a self, raw: &'a [u8], from: &'a Address, rcpt: &'a [Address]) -> BoxFut<'a, Result<()>>;
    /// Push: run until cancelled, sending events. IMAP IDLE; the mock emits nothing.
    fn idle<'a>(&'a self, folder: &'a FolderId, events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>>;
}

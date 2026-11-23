//! The desktop host for the engine: one thread, commands in and events out over channels, timers
//! from [`Event::WakeAt`] delivered as [`Command::Tick`]. The web build hosts the same engine in
//! a Worker with `postMessage` instead.

use crate::{Command, Engine, Event};
use mail_core::{AccountId, FolderId, MailBackend};
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::future::Future;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// What the engine thread receives. Push watchers hold a sender too, so the thread cannot rely
/// on the channel closing and is told to stop instead.
enum Msg { Run(Command), Stop }

pub struct Handle {
    commands: Sender<Msg>,
    pub events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    /// Queue a command. Returns false once the engine has stopped.
    pub fn send(&self, cmd: Command) -> bool { self.commands.send(Msg::Run(cmd)).is_ok() }

    /// Stop after the commands already queued, and wait for the thread.
    pub fn shutdown(mut self) {
        let _ = self.commands.send(Msg::Stop);
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

impl Drop for Handle {
    fn drop(&mut self) { let _ = self.commands.send(Msg::Stop); }
}

/// Start the engine on its own thread. `wake` runs after each batch of events, to wake the UI.
pub fn spawn(mut engine: Engine, wake: impl Fn() + Send + 'static) -> std::io::Result<Handle> {
    let (commands, inbox) = channel::<Msg>();
    let (outbox, events) = channel::<Event>();
    let (pushes, watching) = (commands.clone(), Arc::new(Mutex::new(BTreeSet::new())));
    let thread = thread::Builder::new().name("ravenglass-engine".into()).spawn(move || {
        let mut timers: Vec<f64> = Vec::new();
        loop {
            let next = timers.iter().copied().fold(f64::INFINITY, f64::min);
            let cmd = if next.is_finite() {
                match inbox.recv_timeout(Duration::from_secs_f64((next - now()).max(0.0))) {
                    Ok(Msg::Run(cmd)) => cmd,
                    Err(RecvTimeoutError::Timeout) => { timers.retain(|t| *t > now()); Command::Tick }
                    Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match inbox.recv() { Ok(Msg::Run(cmd)) => cmd, Ok(Msg::Stop) | Err(_) => break }
            };
            let mut sent = false;
            for event in block_on(engine.handle(cmd, now())) {
                match event {
                    Event::WakeAt(t) => timers.push(t),
                    Event::Watch { account, folder } => {
                        if let Some(backend) = engine.backend(&account) { watch(backend, account, folder, pushes.clone(), watching.clone()); }
                    }
                    event => { sent = true; if outbox.send(event).is_err() { return; } }
                }
            }
            if sent { wake(); }
        }
    })?;
    Ok(Handle { commands, events, thread: Some(thread) })
}

/// Hold a push connection (IMAP IDLE) on its own thread and turn each push into a
/// [`Command::Refresh`]. One watcher per account; when the connection ends, the next
/// [`Event::Watch`] starts another.
fn watch(backend: Arc<dyn MailBackend>, account: AccountId, folder: FolderId, commands: Sender<Msg>, watching: Arc<Mutex<BTreeSet<AccountId>>>) {
    if !watching.lock().map(|mut w| w.insert(account.clone())).unwrap_or(false) { return; }
    let spawned = thread::Builder::new().name("ravenglass-push".into()).spawn({
        let (account, watching) = (account.clone(), watching.clone());
        move || {
            let (pushed, pushes) = channel();
            let forward = thread::spawn({
                let account = account.clone();
                move || { for _ in pushes { if commands.send(Msg::Run(Command::Refresh(account.clone()))).is_err() { break; } } }
            });
            // Errors end the watch quietly: the sync machine notices the connection on its own.
            let _ = block_on(backend.idle(&folder, pushed));
            let _ = forward.join();
            if let Ok(mut w) = watching.lock() { w.remove(&account); }
        }
    });
    if spawned.is_err() && let Ok(mut w) = watching.lock() { w.remove(&account); }
}

/// Seconds since the Unix epoch.
pub fn now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) { self.0.unpark(); }
}

/// Run a future on this thread, parking while it waits.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) { Poll::Ready(value) => return value, Poll::Pending => thread::park() }
    }
}

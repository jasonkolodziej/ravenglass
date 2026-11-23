//! The desktop host for the engine: one thread, commands in and events out over channels, timers
//! from [`Event::WakeAt`] delivered as [`Command::Tick`]. The web build hosts the same engine in
//! a Worker with `postMessage` instead.

use crate::{Command, Engine, Event};
use std::future::Future;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Handle {
    commands: Sender<Command>,
    pub events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    /// Queue a command. Returns false once the engine has stopped.
    pub fn send(&self, cmd: Command) -> bool { self.commands.send(cmd).is_ok() }

    /// Stop after the commands already queued, and wait for the thread.
    pub fn shutdown(mut self) {
        let (closed, _) = channel();
        self.commands = closed;
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

/// Start the engine on its own thread. `wake` runs after each batch of events, to wake the UI.
pub fn spawn(mut engine: Engine, wake: impl Fn() + Send + 'static) -> std::io::Result<Handle> {
    let (commands, inbox) = channel::<Command>();
    let (outbox, events) = channel::<Event>();
    let thread = thread::Builder::new().name("ravenglass-engine".into()).spawn(move || {
        let mut timers: Vec<f64> = Vec::new();
        loop {
            let next = timers.iter().copied().fold(f64::INFINITY, f64::min);
            let cmd = if next.is_finite() {
                match inbox.recv_timeout(Duration::from_secs_f64((next - now()).max(0.0))) {
                    Ok(cmd) => cmd,
                    Err(RecvTimeoutError::Timeout) => { timers.retain(|t| *t > now()); Command::Tick }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match inbox.recv() { Ok(cmd) => cmd, Err(_) => break }
            };
            let mut sent = false;
            for event in block_on(engine.handle(cmd, now())) {
                match event {
                    Event::WakeAt(t) => timers.push(t),
                    event => { sent = true; if outbox.send(event).is_err() { return; } }
                }
            }
            if sent { wake(); }
        }
    })?;
    Ok(Handle { commands, events, thread: Some(thread) })
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

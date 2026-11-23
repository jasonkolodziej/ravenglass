//! The app's side of the Command/Event boundary. The UI never touches a backend or the store: it
//! sends [`Command`]s through a [`Link`] and folds the [`Event`]s that come back.
//!
//! Two hosts for the same engine:
//! - **Threaded** (the window): the engine is built and run on its own thread
//!   (`ravenglass_engine::runner`), started after the first frame; events arrive in `poll`.
//! - **Inline** (headless scenes, scripts and tests): each command runs to completion where it is
//!   sent, on a fixed clock, so a one-frame snapshot already shows data and runs are reproducible.

use crate::demo::DemoBackend;
use mail_store::Store;
use ravenglass_engine::runner::{self, Handle};
use ravenglass_engine::{Command, Config, Engine, Event};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// The engine over an in-memory store with the demo accounts.
pub fn demo_engine() -> Result<Engine, String> {
    let store = Store::open_in_memory().map_err(|e| e.to_string())?;
    let mut engine = Engine::new(store, Config::default());
    for backend in DemoBackend::accounts() { engine.add_backend(Arc::new(backend)); }
    Ok(engine)
}

pub enum Link {
    /// Not started, or it failed to start.
    Off,
    Inline { engine: Box<Engine>, now: f64 },
    /// The engine thread is being set up; commands wait in `queued`.
    Starting { ready: Receiver<Result<Handle, String>>, queued: Vec<Command> },
    Running(Handle),
}

impl Link {
    pub fn inline(now: f64) -> Result<Link, String> { Ok(Link::Inline { engine: Box::new(demo_engine()?), now }) }

    /// Build the engine and start its runner off the calling thread. `wake` wakes the UI.
    pub fn start(wake: impl Fn() + Send + Sync + Clone + 'static) -> Link {
        let (tx, ready) = channel();
        let boot = wake.clone();
        let spawned = std::thread::Builder::new().name("ravenglass-boot".into()).spawn(move || {
            let handle = demo_engine().and_then(|engine| runner::spawn(engine, wake).map_err(|e| e.to_string()));
            let _ = tx.send(handle);
            boot();
        });
        match spawned {
            Ok(_) => Link::Starting { ready, queued: Vec::new() },
            Err(_) => Link::Off,
        }
    }

    pub fn is_off(&self) -> bool { matches!(self, Link::Off) }

    /// Send a command. The inline host answers at once; the threaded one answers through [`Link::drain`].
    pub fn send(&mut self, cmd: Command) -> Vec<Event> {
        match self {
            Link::Off => Vec::new(),
            Link::Inline { engine, now } => {
                let mut events = runner::block_on(engine.handle(cmd, *now));
                // Timers have nothing to wait for on a fixed clock.
                events.retain(|e| !matches!(e, Event::WakeAt(_)));
                events
            }
            Link::Starting { queued, .. } => {
                queued.push(cmd);
                Vec::new()
            }
            Link::Running(handle) => {
                if !handle.send(cmd) { *self = Link::Off; }
                Vec::new()
            }
        }
    }

    /// Events that arrived since the last call. Never blocks.
    pub fn drain(&mut self) -> Vec<Event> {
        if let Link::Starting { ready, queued } = self {
            match ready.try_recv() {
                Ok(Ok(handle)) => {
                    for cmd in queued.drain(..) { handle.send(cmd); }
                    *self = Link::Running(handle);
                }
                Ok(Err(reason)) => {
                    *self = Link::Off;
                    return vec![Event::Error { context: "start".into(), message: reason }];
                }
                Err(TryRecvError::Disconnected) => {
                    *self = Link::Off;
                    return vec![Event::Error { context: "start".into(), message: "the engine did not start".into() }];
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        match self {
            Link::Running(handle) => handle.events.try_iter().collect(),
            _ => Vec::new(),
        }
    }
}

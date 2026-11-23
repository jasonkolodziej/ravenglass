//! The app's side of the Command/Event boundary. The UI never touches a backend or the store: it
//! sends [`Command`]s through a [`Link`] and folds the [`Event`]s that come back.
//!
//! Two hosts for the same engine:
//! - **Threaded** (the window): the engine is built and run on its own thread
//!   (`ravenglass_engine::runner`), started after the first frame; events arrive in `poll`.
//! - **Inline** (headless scenes, scripts and tests): each command runs to completion where it is
//!   sent, on a fixed clock, so a one-frame snapshot already shows data and runs are reproducible.

use crate::demo::DemoBackend;
use crate::shell::Services;
use mail_auth::AccountConfig;
use mail_core::AccountId;
use mail_store::Store;
use ravenglass_accounts::{ConnectError, Connected, Connector};
use ravenglass_engine::runner::{self, Handle};
use ravenglass_engine::{Command, Config, Engine, Event};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::Duration;

/// The engine over an in-memory store with the demo accounts.
pub fn demo_engine() -> Result<Engine, String> {
    let store = Store::open_in_memory().map_err(|e| e.to_string())?;
    let mut engine = Engine::new(store, Config::default());
    for backend in DemoBackend::accounts() { engine.add_backend(Arc::new(backend)); }
    // Without a render thread HTML mail still opens, in the text view.
    if let Ok(renderer) = ravenglass_engine::Renderer::spawn() { engine.set_renderer(renderer); }
    Ok(engine)
}

/// What a real engine is built from: the profile's accounts over its store.
#[derive(Clone, Debug)]
pub struct EngineSpec {
    /// `mail.db`; `None` keeps the cache in memory.
    pub store: Option<PathBuf>,
    pub accounts: Vec<AccountConfig>,
    pub undo_send_secs: f64,
}

pub struct Built {
    pub engine: Engine,
    /// The accounts that have a backend, to open.
    pub connected: Vec<(AccountConfig, Connected)>,
    /// Accounts that could not be connected, each with one line that says why. They do not stop
    /// the others.
    pub problems: Vec<(AccountId, String)>,
}

/// One line for an account that has no backend: what is wrong and the command that fixes it.
/// Never contains a secret; `ConnectError` carries none.
pub fn connect_problem(id: &AccountId, error: &ConnectError) -> String {
    match error {
        ConnectError::NoPassword => format!("no password stored · :account password {id}"),
        ConnectError::SignIn => format!("sign-in needed · :account signin {id}"),
        ConnectError::NoClientId(provider) => format!("this build has no {} OAuth client id", crate::accounts_ui::provider_name(*provider)),
        ConnectError::Invalid(problem) => format!("invalid settings: {problem}"),
        other => other.to_string().lines().next().unwrap_or("could not connect").to_owned(),
    }
}

/// The engine over the profile's store with every account that connects (architecture §3).
/// Nothing touches the network here but an OAuth token refresh: backends connect on first use.
pub fn real_engine(spec: &EngineSpec, services: &Services, now: f64) -> Result<Built, String> {
    let store = match &spec.store {
        Some(path) => Store::open(&path.to_string_lossy()),
        None => Store::open_in_memory(),
    }
    .map_err(|e| format!("mail.db: {e}"))?;
    let mut engine = Engine::new(store, Config { undo_send_secs: spec.undo_send_secs });
    let connector = services.connector();
    let (mut connected, mut problems) = (Vec::new(), Vec::new());
    for account in &spec.accounts {
        match connector.connect(account, now) {
            Ok(link) => {
                engine.add_backend(link.backend.clone());
                engine.set_identity(&account.id, link.identity.clone());
                connected.push((account.clone(), link));
            }
            Err(error) => problems.push((account.id.clone(), connect_problem(&account.id, &error))),
        }
    }
    // Without a render thread HTML mail still opens, in the text view.
    if let Ok(renderer) = ravenglass_engine::Renderer::spawn() { engine.set_renderer(renderer); }
    engine.set_image_fetcher(ravenglass_engine::fetch::https_images());
    Ok(Built { engine, connected, problems })
}

/// What opens a freshly built engine: every connected account, then the drafts kept from before.
pub fn first_commands(connected: &[(AccountConfig, Connected)]) -> Vec<Command> {
    let mut out: Vec<Command> = connected.iter().map(|(account, _)| Command::OpenAccount(account.id.clone())).collect();
    out.push(Command::LoadDrafts);
    out
}

/// Keep OAuth access tokens fresh on a thread of their own: each is traded for a new one a
/// minute before it lapses. The thread ends when the returned sender is dropped. `None` when no
/// account uses OAuth.
pub fn keep_tokens_fresh(connector: Connector, mut connected: Vec<(AccountConfig, Connected)>) -> Option<Sender<()>> {
    connected.retain(|(_, link)| link.token_expires_at.is_some());
    if connected.is_empty() { return None; }
    let (guard, stop) = channel::<()>();
    let refresher = move || loop {
        let now = runner::now();
        let next = connected.iter().filter_map(|(_, link)| link.token_expires_at).fold(f64::INFINITY, f64::min);
        let wait = if next.is_finite() { (next - 60.0 - now).clamp(5.0, 3600.0) } else { 300.0 };
        match stop.recv_timeout(Duration::from_secs_f64(wait)) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
        }
        let now = runner::now();
        for (account, link) in &mut connected {
            if link.token_expires_at.is_some_and(|at| now >= at - 60.0) && connector.refresh(link, account, now).is_err() {
                // Try again in five minutes; a sign-in that is truly gone shows as "sign-in failed".
                link.token_expires_at = Some(now + 360.0);
            }
        }
    };
    std::thread::Builder::new().name("ravenglass-tokens".into()).spawn(refresher).ok()?;
    Some(guard)
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

    /// Build the demo engine and start its runner off the calling thread. `wake` wakes the UI.
    pub fn start(wake: impl Fn() + Send + Sync + Clone + 'static) -> Link {
        Link::start_with(|| demo_engine().map(|engine| (engine, Vec::new())), wake)
    }

    /// Run `build` off the calling thread and start the runner over the engine it returns. The
    /// commands it returns go first; commands sent meanwhile wait in the queue behind them.
    pub fn start_with(build: impl FnOnce() -> Result<(Engine, Vec<Command>), String> + Send + 'static, wake: impl Fn() + Send + Sync + Clone + 'static) -> Link {
        let (tx, ready) = channel();
        let boot = wake.clone();
        let spawned = std::thread::Builder::new().name("ravenglass-boot".into()).spawn(move || {
            let handle = build().and_then(|(engine, first)| {
                let handle = runner::spawn(engine, wake).map_err(|e| e.to_string())?;
                for cmd in first { handle.send(cmd); }
                Ok(handle)
            });
            let _ = tx.send(handle);
            boot();
        });
        match spawned {
            Ok(_) => Link::Starting { ready, queued: Vec::new() },
            Err(_) => Link::Off,
        }
    }

    /// Stop the engine and wait for its thread, so the store's file is closed. Call it off the
    /// UI thread: a command in flight (a sync) is finished first.
    pub fn shutdown(self) {
        match self {
            Link::Running(handle) => handle.shutdown(),
            Link::Starting { ready, .. } => if let Ok(Ok(handle)) = ready.recv() { handle.shutdown(); },
            Link::Inline { .. } | Link::Off => {}
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

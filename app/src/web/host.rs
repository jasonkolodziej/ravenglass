//! The web build's host: what `main.rs` does on desktop, for a page. It owns the Relay session
//! ([`Session`]), shows the sign-in screen until there is one, then builds the engine over the
//! Relay's accounts and hands everything else to the desktop app's own `poll`, `on_key` and view.
//!
//! The engine runs on the page thread as a `spawn_local` task (`ravenglass_engine::runner` on
//! wasm), its backends speak `fetch` and WebSocket (`relay_client::web`), and HTML mail is laid
//! out inline. The mail cache is SQLite in memory for now: the persistent VFS (OPFS) needs the
//! engine in a Worker (architecture §6), so a reload syncs again from the Relay.

use super::screen;
use super::session::{self, Effect, Event, Grant, Offer, Problem, Session, Signed};
use crate::keys::KeyPress;
use crate::link::Link;
use crate::platform::web::{self as browser, Keep};
use crate::state::{AppState, Boot, Source, Workspace};
use mail_core::AccountId;
use mail_core::machine::account_sync;
use ravenglass_engine::{Config, Engine, runner};
use relay_client::web::{self as relay, Auth, AuthMode, SignInError, WebBackend, WebTransport};
use rune::{AppSpec, Cx, El, KeyEvent, Tx};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// `sessionStorage`: a single-user Relay's token, for this tab only.
const TOKEN_KEY: &str = "ravenglass.relay-token";
/// `localStorage`: `config.toml`'s text, the web's settings file.
const CONFIG_KEY: &str = "ravenglass.config";

/// The web's part of [`AppState`].
#[derive(Default)]
pub struct Web {
    pub session: Session,
    /// The Relay's origin; empty for the page's own.
    pub relay: String,
    /// Answers from sign-in requests, folded on the next poll.
    answers: Rc<RefCell<Vec<Event>>>,
    /// The start-up check is on its way.
    checking: bool,
    /// The settings as last written to storage.
    saved: String,
}

pub fn spec() -> AppSpec<AppState> {
    AppSpec::<AppState> { poll: Some(poll), on_key: Some(on_key), view: root, init: launch, ..crate::spec() }
}

/// The page's first state: the sign-in check pending, the settings from browser storage.
fn launch() -> AppState {
    let mut s = AppState::windowed();
    // The engine starts once signed in, from here; never the demo's.
    s.boot = Boot::Started;
    s.web.relay = browser::relay_origin();
    load_settings(&mut s);
    s
}

fn root(s: &AppState, cx: &mut Cx<AppState>) -> El {
    if s.web.session.is_ready() { return crate::view::root(s, cx); }
    screen::root(s, &s.web.session, &browser::relay_label(&s.web.relay), cx)
}

fn poll(s: &mut AppState, clock: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if !s.web.checking && let Some(waker) = s.waker.clone() {
        s.web.checking = true;
        let relay = s.web.relay.clone();
        ask(s, waker, async move {
            let (result, offer) = check(&relay).await;
            Event::Checked(result, offer)
        });
    }
    let answers = std::mem::take(&mut *s.web.answers.borrow_mut());
    for event in answers {
        changed = true;
        apply(s, event);
    }
    changed |= crate::poll(s, clock, tx);
    if relay_refused(s) {
        apply(s, Event::SignedOut);
        changed = true;
    }
    save_settings(s);
    mirror(s);
    changed
}

fn on_key(s: &mut AppState, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.web.session.is_ready() { return crate::on_key(s, ev, tx); }
    let Some(press) = KeyPress::from_event(ev) else { return false };
    apply(s, Event::Key(press));
    mirror(s);
    true
}

/// Step the session machine and carry out what it asks.
fn apply(s: &mut AppState, event: Event) {
    for effect in session::step(&mut s.web.session, event) {
        match effect {
            Effect::Login { user, password } => {
                let (relay, Some(waker)) = (s.web.relay.clone(), s.waker.clone()) else { continue };
                ask(s, waker, async move { Event::Answered(login(&relay, &user, &password).await) });
            }
            Effect::UseToken(token) => {
                let (relay, Some(waker)) = (s.web.relay.clone(), s.waker.clone()) else { continue };
                ask(s, waker, async move { Event::Answered(by_token(&relay, token).await) });
            }
            Effect::Oidc => browser::navigate(&relay::oidc_start(&s.web.relay)),
            Effect::KeepToken(token) => browser::write(Keep::Tab, TOKEN_KEY, &token),
            Effect::Start(signed) => start(s, signed),
            Effect::Forget => {
                browser::forget(Keep::Tab, TOKEN_KEY);
                s.link = Link::Off;
            }
        }
    }
}

/// Every account's sync stopped at sign-in: the Relay no longer takes this session (it expired,
/// or the token changed), rather than one mail server refusing its password.
fn relay_refused(s: &AppState) -> bool {
    let Session::Ready(signed) = &s.web.session else { return false };
    !signed.accounts.is_empty() && signed.accounts.iter().all(|a| s.mail.sync.get(a) == Some(&account_sync::State::AuthFailed))
}

/// Run a Relay request as a task; its answer is folded on the poll it wakes.
fn ask(s: &AppState, wake: rune::Waker, work: impl Future<Output = Event> + 'static) {
    let answers = s.web.answers.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let event = work.await;
        answers.borrow_mut().push(event);
        wake();
    });
}

/// At start: what the Relay offers, then a token kept by this tab, else a cookie session, else
/// the sign-in screen with the forms the Relay takes.
async fn check(relay: &str) -> (Result<Signed, Problem>, Offer) {
    let offer = match relay::auth_config(relay).await {
        Ok(Some(config)) if config.mode == AuthMode::Token => Offer { password: false, token: true, oidc: false },
        Ok(Some(config)) => Offer { password: config.password, token: false, oidc: config.oidc },
        Ok(None) => Offer::default(),
        Err(e) => return (Err(problem(e)), Offer::default()),
    };
    if offer.token && let Some(token) = browser::read(Keep::Tab, TOKEN_KEY).filter(|t| !t.is_empty()) {
        let checked = by_token(relay, token).await;
        if matches!(checked, Err(Problem::Refused)) { browser::forget(Keep::Tab, TOKEN_KEY); }
        return (checked, offer);
    }
    if !offer.password && !offer.oidc { return (Err(Problem::Refused), offer); }
    let checked = match relay::session(relay).await.map_err(problem) {
        Ok(signed) => with_accounts(relay, signed).await,
        Err(e) => Err(e),
    };
    (checked, offer)
}

async fn login(relay: &str, user: &str, password: &str) -> Result<Signed, Problem> {
    let signed = relay::login(relay, user, password).await.map_err(problem)?;
    with_accounts(relay, signed).await
}

async fn by_token(relay: &str, token: String) -> Result<Signed, Problem> {
    let opened = relay::check_token(relay, &token).await.map_err(problem)?;
    Ok(Signed { grant: Grant::Token(token), who: "relay token".into(), accounts: opened.accounts })
}

/// A cookie session's accounts, from the protocol session it opens.
async fn with_accounts(relay: &str, signed: relay::SignedIn) -> Result<Signed, Problem> {
    let probe = relay_client::RelayBackend::new(AccountId::from(""), WebTransport::new(relay, Auth::Session { csrf: signed.csrf.clone() }));
    let opened = probe.open_session().await.map_err(|e| match e {
        mail_core::MailError::Auth => Problem::Refused,
        mail_core::MailError::Network(m) => Problem::Offline(m),
        other => Problem::Relay(other.to_string()),
    })?;
    Ok(Signed { grant: Grant::Cookie { csrf: signed.csrf }, who: signed.user, accounts: opened.accounts })
}

fn problem(e: SignInError) -> Problem {
    match e {
        SignInError::Unauthorized => Problem::Refused,
        SignInError::RateLimited(secs) => Problem::Busy(secs),
        SignInError::Network(m) => Problem::Offline(m),
        SignInError::Server(m) => Problem::Relay(m),
    }
}

/// Signed in: the engine over the Relay's accounts, started on this thread.
fn start(s: &mut AppState, signed: Signed) {
    let Some(waker) = s.waker.clone() else { return };
    let auth = match signed.grant { Grant::Cookie { csrf } => Auth::Session { csrf }, Grant::Token(token) => Auth::Token(token) };
    let store = match mail_store::Store::open_in_memory() {
        Ok(store) => store,
        Err(e) => return s.error(format!("mail cache: {e}")),
    };
    let mut engine = Engine::new(store, Config { undo_send_secs: f64::from(s.shell.config.ui.undo_send_seconds) });
    for account in &signed.accounts {
        engine.add_backend(Arc::new(WebBackend::new(account.clone(), WebTransport::new(&s.web.relay, auth.clone()))));
    }
    // HTML mail is laid out on this thread; without a renderer it opens in the text view.
    if let Ok(renderer) = ravenglass_engine::Renderer::spawn() { engine.set_renderer(renderer); }
    match runner::spawn(engine, move || waker()) {
        Ok(handle) => s.link = Link::Running(handle),
        Err(e) => return s.error(format!("engine: {e}")),
    }
    s.workspaces = workspaces(&s.workspaces, &signed.accounts);
    s.active = 0;
    if signed.accounts.is_empty() {
        // A new user of a multi-user Relay: their accounts are added on the Relay (`/v1/me/accounts`).
        s.toast("signed in · this relay has no mail accounts for you yet");
    } else {
        s.open_accounts();
    }
    browser::persist_storage();
}

/// The workspaces for the Relay's accounts: the ones already set up when every account they
/// name is served (the demo layout, against a demo Relay), else one Inbox per account.
fn workspaces(current: &[Workspace], accounts: &[AccountId]) -> Vec<Workspace> {
    let served = |w: &Workspace| match &w.source { Source::Mailbox { account, .. } => accounts.contains(account), Source::Search(_) => true };
    if !current.is_empty() && current.iter().all(served) {
        return current.iter().map(|w| Workspace::new(w.slot, &w.name, w.source.clone())).collect();
    }
    let mut out: Vec<Workspace> = accounts.iter().take(9).enumerate().map(|(i, a)| Workspace::new(i as u8 + 1, &a.0, Source::Mailbox { account: a.clone(), name: "Inbox".into() })).collect();
    if out.is_empty() { out.push(Workspace::new(1, "welcome", Source::Mailbox { account: AccountId::from("ravenglass"), name: "welcome".into() })); }
    out
}

/// `config.toml` from `localStorage`: the theme, keys and the rest of the settings.
fn load_settings(s: &mut AppState) {
    let Some(text) = browser::read(Keep::Settings, CONFIG_KEY) else { return };
    match ravenglass_config::Config::parse(&text) {
        Ok(config) => {
            s.shell.config = config;
            let problem = crate::keymap::apply(&s.shell.config.keys);
            let wanted = s.shell.config.theme_for(true).to_owned();
            if let Some(theme) = s.find_theme(&wanted) { s.theme = theme; }
            if let Some(problem) = problem { s.error(problem); }
        }
        Err(e) => s.error(format!("settings: {}", e.to_string().lines().next().unwrap_or("unreadable"))),
    }
    s.web.saved = text;
}

/// Write the settings back when they changed (`:theme` does).
fn save_settings(s: &mut AppState) {
    let text = s.shell.config.to_toml();
    if text != s.web.saved {
        browser::write(Keep::Settings, CONFIG_KEY, &text);
        s.web.saved = text;
    }
}

/// The page's live region and `data-rg-*` marks: what a screen reader hears, and what a test
/// driving the page can read without pixels.
fn mirror(s: &AppState) {
    let state = match &s.web.session {
        Session::Checking => "checking",
        Session::SignIn(form) if form.error.is_some() => "signin-error",
        Session::SignIn(_) => "signin",
        Session::SigningIn(_) => "signing-in",
        Session::Ready(_) => "ready",
    };
    browser::mark("session", state);
    if !s.web.session.is_ready() {
        let line = s.web.session.form().and_then(|f| f.error.clone()).unwrap_or_else(|| "Sign in to the relay".into());
        return browser::announce(&line);
    }
    let rows = s.rows().len();
    browser::mark("rows", &rows.to_string());
    browser::mark("selected", &s.ws().sel.to_string());
    let open = s.ws().open.as_ref();
    browser::mark("open", &open.map(|e| e.id.0.clone()).unwrap_or_default());
    browser::mark("body", if open.is_some() && s.body().is_some() { "shown" } else { "" });
    let line = match open {
        Some(e) => format!("{} · reading: {}", s.ws().name, e.subject),
        None => format!("{} · {rows} messages", s.ws().name),
    };
    browser::announce(&line);
}

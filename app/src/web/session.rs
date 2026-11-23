//! Signing in to the Relay (architecture §8, "Authentication on the web"), as one state machine:
//! an enum [`Session`] and a pure [`step`] that returns [`Effect`]s for the browser host to carry
//! out. Two kinds of Relay are signed in to the same way from here:
//!
//! - a multi-user Relay: username and password, answered with a cookie session;
//! - a single-user Relay: its shared token, typed once and kept for the tab only.
//!
//! Nothing here touches the network or storage, so every transition is unit-tested on desktop.

use crate::accounts_ui::Secret;
use crate::keys::{K, KeyPress};
use mail_core::AccountId;

/// How the requests of a signed-in session are authorised.
#[derive(Clone, PartialEq, Eq)]
pub enum Grant {
    /// The Relay's `HttpOnly` cookie, which the browser keeps; `csrf` goes on every change.
    Cookie { csrf: String },
    /// A single-user Relay's bearer token.
    Token(String),
}

impl std::fmt::Debug for Grant {
    /// Never prints a secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Grant::Cookie { .. } => "Grant::Cookie", Grant::Token(_) => "Grant::Token" })
    }
}

/// A working session: who, how, and the accounts the Relay serves them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub grant: Grant,
    pub who: String,
    pub accounts: Vec<AccountId>,
}

/// Why a check or a sign-in gave no session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The Relay said no (401): not signed in, wrong password, wrong token.
    Refused,
    /// The Relay could not be reached.
    Offline(String),
    /// Too many attempts (429); try again after this many seconds, if the Relay said.
    Busy(Option<u32>),
    /// Anything else the Relay answered.
    Relay(String),
}

/// The sign-in methods a Relay offers (`GET /v1/auth/config`): a single-user Relay takes its
/// token, a multi-user one a username and password and/or an identity provider (OIDC). A Relay
/// that does not say is offered both forms, without OIDC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer { pub password: bool, pub token: bool, pub oidc: bool }

impl Default for Offer {
    fn default() -> Offer { Offer { password: true, token: true, oidc: false } }
}

impl Offer {
    /// The form's fields for this offer, in focus order.
    pub fn fields(self) -> Vec<Field> {
        let mut out = Vec::new();
        if self.password { out.extend([Field::User, Field::Password]); }
        if self.oidc { out.push(Field::Oidc); }
        if self.token || out.is_empty() { out.push(Field::Token); }
        out
    }
}

/// The sign-in form's fields. `Oidc` is the "sign in with your identity provider" line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Field { #[default] User, Password, Oidc, Token }

/// The longest username taken, in characters.
const MAX_USER: usize = 128;

pub struct Form {
    pub offer: Offer,
    pub field: Field,
    pub user: String,
    pub password: Secret,
    pub token: Secret,
    /// One line under the fields; never contains what was typed.
    pub error: Option<String>,
}

impl Default for Form {
    fn default() -> Form { Form::new(Offer::default(), None) }
}

impl Form {
    fn new(offer: Offer, error: Option<String>) -> Form {
        Form { offer, field: offer.fields()[0], user: String::new(), password: Secret::new(), token: Secret::new(), error }
    }

    fn next(&mut self, back: bool) {
        let fields = self.offer.fields();
        let at = fields.iter().position(|f| *f == self.field).unwrap_or(0);
        self.field = fields[(at + if back { fields.len() - 1 } else { 1 }) % fields.len()];
    }

    /// The typed secrets go as soon as they have been sent.
    fn wipe(&mut self) {
        self.password.wipe();
        self.token.wipe();
    }
}

#[derive(Default)]
pub enum Session {
    /// Asking the Relay whether this tab already has a session (a cookie, or a kept token).
    #[default]
    Checking,
    SignIn(Form),
    /// Credentials are on their way; keys wait.
    SigningIn(Form),
    Ready(Signed),
}

impl Session {
    pub fn is_ready(&self) -> bool { matches!(self, Session::Ready(_)) }

    pub fn form(&self) -> Option<&Form> {
        match self { Session::SignIn(form) | Session::SigningIn(form) => Some(form), _ => None }
    }
}

pub enum Event {
    /// The answer to the check made at start, with what the Relay offers.
    Checked(Result<Signed, Problem>, Offer),
    Key(KeyPress),
    /// The answer to a sign-in.
    Answered(Result<Signed, Problem>),
    /// The Relay no longer takes this session (a 401 mid-session) or the user signed out.
    SignedOut,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// `POST /v1/auth/login`.
    Login { user: String, password: String },
    /// Open a protocol session with this bearer token.
    UseToken(String),
    /// Leave for the Relay's OIDC sign-in (`/v1/auth/oidc/start`); it comes back signed in.
    Oidc,
    /// Build the engine over these accounts.
    Start(Signed),
    /// Keep the token for this tab (`sessionStorage`), so a reload does not ask again.
    KeepToken(String),
    /// Drop the kept token and stop the engine.
    Forget,
}

/// One transition. Events that make no sense in a state are ignored and return nothing.
pub fn step(state: &mut Session, event: Event) -> Vec<Effect> {
    let current = std::mem::take(state);
    let (next, effects) = match (current, event) {
        (Session::Checking, Event::Checked(Ok(signed), _)) => (Session::Ready(signed.clone()), vec![Effect::Start(signed)]),
        // No session yet is the usual start; only a Relay that cannot be reached is worth a line.
        (Session::Checking, Event::Checked(Err(problem), offer)) => (Session::SignIn(Form::new(offer, match problem {
            Problem::Refused => None,
            other => Some(describe(&other, false)),
        })), Vec::new()),
        (Session::SignIn(mut form), Event::Key(press)) => {
            let effects = key(&mut form, press);
            if effects.is_empty() { (Session::SignIn(form), effects) } else { form.error = None; (Session::SigningIn(form), effects) }
        }
        (Session::SigningIn(mut form), Event::Answered(Ok(signed))) => {
            form.wipe();
            let mut effects = Vec::new();
            if let Grant::Token(token) = &signed.grant { effects.push(Effect::KeepToken(token.clone())); }
            effects.push(Effect::Start(signed.clone()));
            (Session::Ready(signed), effects)
        }
        (Session::SigningIn(mut form), Event::Answered(Err(problem))) => {
            let by_token = form.field == Field::Token;
            form.password.wipe();
            form.token.wipe();
            form.error = Some(describe(&problem, by_token));
            (Session::SignIn(form), Vec::new())
        }
        (Session::Ready(signed), Event::SignedOut) => {
            let offer = match signed.grant { Grant::Token(_) => Offer { password: false, token: true, oidc: false }, Grant::Cookie { .. } => Offer::default() };
            (Session::SignIn(Form::new(offer, Some("signed out of the relay".into()))), vec![Effect::Forget])
        }
        (other, _) => (other, Vec::new()),
    };
    *state = next;
    effects
}

/// A key on the form. Returns the sign-in to send, if `enter` completed one.
fn key(form: &mut Form, press: KeyPress) -> Vec<Effect> {
    if press.ctrl || press.cmd { return Vec::new(); }
    match press.key {
        K::Tab => form.next(press.shift),
        K::Down => form.next(false),
        K::Up => form.next(true),
        K::Backspace => match form.field {
            Field::User => { form.user.pop(); }
            Field::Password => form.password.pop(),
            Field::Token => form.token.pop(),
            Field::Oidc => {}
        },
        K::Esc => form.error = None,
        K::Char(c) if !c.is_control() => match form.field {
            Field::User => if form.user.chars().count() < MAX_USER { form.user.push(c) },
            Field::Password => form.password.push(c),
            Field::Token => form.token.push(c),
            Field::Oidc => {}
        },
        K::Enter => return submit(form),
        _ => {}
    }
    Vec::new()
}

/// `enter`: the token if the token field is the one in use, else username and password.
fn submit(form: &mut Form) -> Vec<Effect> {
    if form.field == Field::Oidc { return vec![Effect::Oidc]; }
    let by_token = form.field == Field::Token || !form.offer.password || (form.user.trim().is_empty() && !form.token.is_empty());
    if by_token {
        if form.token.is_empty() {
            form.error = Some("type the relay's token".into());
            return Vec::new();
        }
        form.field = Field::Token;
        return vec![Effect::UseToken(form.token.expose().trim().to_owned())];
    }
    if form.user.trim().is_empty() {
        form.field = Field::User;
        form.error = Some("type your username".into());
        return Vec::new();
    }
    if form.password.is_empty() {
        form.field = Field::Password;
        form.error = Some("type your password".into());
        return Vec::new();
    }
    vec![Effect::Login { user: form.user.trim().to_owned(), password: form.password.expose().to_owned() }]
}

fn describe(problem: &Problem, by_token: bool) -> String {
    match problem {
        Problem::Refused if by_token => "the relay did not take that token".into(),
        Problem::Refused => "wrong username or password".into(),
        Problem::Busy(Some(secs)) => format!("too many attempts · try again in {secs} s"),
        Problem::Busy(None) => "too many attempts · try again later".into(),
        Problem::Offline(why) => format!("the relay cannot be reached · {}", first_line(why)),
        Problem::Relay(why) => format!("the relay refused · {}", first_line(why)),
    }
}

fn first_line(text: &str) -> &str { text.lines().next().unwrap_or("").trim() }

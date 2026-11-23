//! Adding an account, as a state machine (`mail-core::machine` style): an enum [`Flow`] and a
//! [`step`] that returns what should happen instead of doing it. The network and the disk are
//! someone else's job (`shell.rs` runs the effects off the UI thread and feeds the answers back).
//!
//! `:account add you@example.com` → settings are discovered → the form shows them (guesses are
//! marked and every field can be edited) → a masked password prompt, or the browser sign-in for
//! OAuth providers → the settings are saved and the secret goes to the `SecretStore`.
//!
//! The password lives in a [`Secret`]: never echoed, never in `Debug`, wiped when dropped.

use crate::keys::{K, KeyPress};
use mail_auth::autoconfig::{Settings, Source};
use mail_auth::oauth::Provider;
use mail_auth::{AccountConfig, AuthKind, Incoming, Security, Server};
use mail_core::AccountId;

/// Text that must not be shown or logged. It never reallocates, so wiping it on drop leaves no
/// copy behind in the heap.
pub struct Secret(String);

impl Secret {
    /// The longest password taken, in characters.
    pub const MAX: usize = 256;

    pub fn new() -> Secret { Secret(String::with_capacity(Secret::MAX * 4)) }

    pub fn push(&mut self, c: char) {
        if self.len() < Secret::MAX && !c.is_control() { self.0.push(c); }
    }

    pub fn pop(&mut self) { self.0.pop(); }
    pub fn len(&self) -> usize { self.0.chars().count() }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    /// The secret itself: for the `SecretStore` and the password check only.
    pub fn expose(&self) -> &str { &self.0 }

    /// What the screen shows: one `•` per character.
    pub fn masked(&self) -> String { "•".repeat(self.len()) }

    pub fn wipe(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        std::hint::black_box(&bytes);
        bytes.clear();
        self.0 = String::from_utf8(bytes).unwrap_or_default();
    }
}

impl Default for Secret {
    fn default() -> Self { Secret::new() }
}

impl Drop for Secret {
    fn drop(&mut self) { self.wipe(); }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Secret(…)") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKey { Name, Username, ImapHost, ImapPort, ImapSecurity, SmtpHost, SmtpPort, SmtpSecurity, JmapUrl, Auth }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    /// Digits only.
    Port,
    /// One of a fixed list; space and the arrow keys change it.
    Choice(&'static [&'static str]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub key: FieldKey,
    pub label: &'static str,
    pub value: String,
    pub kind: FieldKind,
}

const SECURITY: &[&str] = &["TLS", "STARTTLS", "none (this machine only)"];
const AUTH: &[&str] = &["password", "Google sign-in", "Microsoft sign-in"];
/// The longest value a text field takes.
const MAX_FIELD: usize = 200;

fn security_label(s: Security) -> &'static str { SECURITY[match s { Security::Tls => 0, Security::StartTls => 1, Security::Plain => 2 }] }

fn security_from(label: &str) -> Security {
    match SECURITY.iter().position(|l| *l == label) { Some(1) => Security::StartTls, Some(2) => Security::Plain, _ => Security::Tls }
}

fn auth_label(a: &AuthKind) -> &'static str {
    AUTH[match a { AuthKind::Password => 0, AuthKind::OAuth2(Provider::Google) => 1, AuthKind::OAuth2(Provider::Microsoft) => 2 }]
}

fn auth_from(label: &str) -> AuthKind {
    match AUTH.iter().position(|l| *l == label) { Some(1) => AuthKind::OAuth2(Provider::Google), Some(2) => AuthKind::OAuth2(Provider::Microsoft), _ => AuthKind::Password }
}

pub fn provider_name(p: Provider) -> &'static str { match p { Provider::Google => "Google", Provider::Microsoft => "Microsoft" } }

/// The environment variable a build reads its OAuth client id from.
pub fn client_id_env(p: Provider) -> &'static str {
    match p { Provider::Google => "RAVENGLASS_GOOGLE_CLIENT_ID", Provider::Microsoft => "RAVENGLASS_MICROSOFT_CLIENT_ID" }
}

/// Google still takes app passwords (with 2-step verification on); Microsoft has turned password
/// sign-in off for IMAP and SMTP.
pub fn allows_app_password(p: Provider) -> bool { p == Provider::Google }

/// The settings of one account while they are being reviewed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    pub id: AccountId,
    pub address: String,
    /// Where the settings came from. `None`: an account that already exists.
    pub source: Option<Source>,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub server_saves_sent: bool,
    /// Why the last attempt to go on failed.
    pub error: Option<String>,
}

impl Form {
    pub fn from_settings(id: AccountId, address: &str, settings: Settings) -> Form {
        let source = settings.source;
        let mut form = Form::from_config(&settings.into_account(id, "", address));
        form.source = Some(source);
        form
    }

    pub fn from_config(config: &AccountConfig) -> Form {
        let field = |key, label, value: String, kind| Field { key, label, value, kind };
        let mut fields = vec![field(FieldKey::Name, "name", config.name.clone(), FieldKind::Text), field(FieldKey::Username, "user name", config.username.clone(), FieldKind::Text)];
        let server = |fields: &mut Vec<Field>, s: &Server, keys: [FieldKey; 3], labels: [&'static str; 3]| {
            fields.push(field(keys[0], labels[0], s.host.clone(), FieldKind::Text));
            fields.push(field(keys[1], labels[1], s.port.to_string(), FieldKind::Port));
            fields.push(field(keys[2], labels[2], security_label(s.security).into(), FieldKind::Choice(SECURITY)));
        };
        match &config.incoming {
            Incoming::Imap(imap) => server(&mut fields, imap, [FieldKey::ImapHost, FieldKey::ImapPort, FieldKey::ImapSecurity], ["imap server", "imap port", "imap security"]),
            Incoming::Jmap { session_url } => fields.push(field(FieldKey::JmapUrl, "jmap session", session_url.clone(), FieldKind::Text)),
        }
        if let Some(smtp) = &config.smtp { server(&mut fields, smtp, [FieldKey::SmtpHost, FieldKey::SmtpPort, FieldKey::SmtpSecurity], ["smtp server", "smtp port", "smtp security"]); }
        fields.push(field(FieldKey::Auth, "sign-in", auth_label(&config.auth).into(), FieldKind::Choice(AUTH)));
        Form { id: config.id.clone(), address: config.address.clone(), source: None, fields, focus: 0, server_saves_sent: config.server_saves_sent, error: None }
    }

    fn value(&self, key: FieldKey) -> Option<&str> { self.fields.iter().find(|f| f.key == key).map(|f| f.value.trim()) }

    pub fn auth(&self) -> AuthKind { auth_from(self.value(FieldKey::Auth).unwrap_or("")) }

    fn set_auth(&mut self, auth: &AuthKind) {
        if let Some(f) = self.fields.iter_mut().find(|f| f.key == FieldKey::Auth) { f.value = auth_label(auth).into(); }
    }

    /// The account these fields describe, or the first thing wrong with them.
    pub fn config(&self) -> Result<AccountConfig, String> {
        let server = |host: FieldKey, port: FieldKey, security: FieldKey, what: &str| -> Result<Option<Server>, String> {
            let Some(host) = self.value(host) else { return Ok(None) };
            if host.is_empty() || host.contains(char::is_whitespace) { return Err(format!("the {what} server needs a host name")); }
            let port: u16 = self.value(port).unwrap_or("").parse().ok().filter(|p| *p > 0).ok_or(format!("the {what} port is not a port number"))?;
            Ok(Some(Server { host: host.to_owned(), port, security: security_from(self.value(security).unwrap_or("")) }))
        };
        let incoming = match self.value(FieldKey::JmapUrl) {
            Some(url) => Incoming::Jmap { session_url: url.to_owned() },
            None => Incoming::Imap(server(FieldKey::ImapHost, FieldKey::ImapPort, FieldKey::ImapSecurity, "IMAP")?.ok_or("the account has no incoming server")?),
        };
        let config = AccountConfig {
            id: self.id.clone(),
            name: self.value(FieldKey::Name).unwrap_or("").to_owned(),
            address: self.address.clone(),
            username: self.value(FieldKey::Username).unwrap_or("").to_owned(),
            incoming,
            smtp: server(FieldKey::SmtpHost, FieldKey::SmtpPort, FieldKey::SmtpSecurity, "SMTP")?,
            auth: self.auth(),
            server_saves_sent: self.server_saves_sent,
        };
        match config.problem() { Some(problem) => Err(problem.to_owned()), None => Ok(config) }
    }

    fn edit(&mut self, key: KeyPress) {
        let count = self.fields.len();
        let Some(field) = self.fields.get_mut(self.focus) else { return };
        match (key.key, field.kind) {
            (K::Down | K::Tab, _) if !key.shift => self.focus = (self.focus + 1) % count,
            (K::Up | K::Tab, _) => self.focus = (self.focus + count - 1) % count,
            (K::Left, FieldKind::Choice(options)) => cycle(field, options, false),
            (K::Right | K::Char(' '), FieldKind::Choice(options)) => cycle(field, options, true),
            (K::Backspace, FieldKind::Text | FieldKind::Port) => { field.value.pop(); }
            (_, FieldKind::Text) => if let Some(c) = key.typed() && field.value.chars().count() < MAX_FIELD { field.value.push(c); },
            (_, FieldKind::Port) => if let Some(c) = key.typed().filter(char::is_ascii_digit) && field.value.len() < 5 { field.value.push(c); },
            _ => {}
        }
    }
}

fn cycle(field: &mut Field, options: &[&str], forward: bool) {
    let at = options.iter().position(|o| *o == field.value).unwrap_or(0);
    let next = if forward { (at + 1) % options.len() } else { (at + options.len() - 1) % options.len() };
    field.value = options[next].to_owned();
}

#[derive(Debug)]
pub enum Flow {
    /// Looking the address's settings up.
    Discovering { id: AccountId, email: String },
    Review { form: Form },
    /// The masked password prompt. `existing`: a new password for an account already saved.
    Password { form: Form, input: Secret, existing: bool },
    /// An OAuth provider, and this build has no client id for it.
    NoClientId { form: Form, provider: Provider },
    /// The browser has the sign-in page; waiting for its redirect.
    SigningIn { form: Form },
    Saving { form: Form },
}

impl Flow {
    pub fn form(&self) -> Option<&Form> {
        match self {
            Flow::Discovering { .. } => None,
            Flow::Review { form } | Flow::Password { form, .. } | Flow::NoClientId { form, .. } | Flow::SigningIn { form } | Flow::Saving { form } => Some(form),
        }
    }
}

#[derive(Debug)]
pub enum FlowEvent {
    Key(KeyPress),
    Discovered(Result<Settings, String>),
    SignedIn(Result<(), String>),
    Saved(Result<(), String>),
}

#[derive(Debug)]
pub enum FlowEffect {
    /// Write the settings and, for a password account, store the password.
    Save { config: AccountConfig, password: Option<Secret> },
    /// Run the browser sign-in for this account.
    SignIn(AccountConfig),
    /// Give up a sign-in that is waiting for the browser.
    CancelSignIn,
    /// The flow ended; say this in a toast (`error` picks its colour).
    Closed { text: String, error: bool },
}

/// Which OAuth providers this build has a client id for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Providers { pub google: bool, pub microsoft: bool }

impl Providers {
    fn has(self, p: Provider) -> bool { match p { Provider::Google => self.google, Provider::Microsoft => self.microsoft } }
}

/// An id for a new account: the first label of the address's domain (`icloud`, `gmail`), made
/// unique with a number. Ids name keyring entries and never contain the address itself.
pub fn new_account_id(address: &str, taken: &[AccountId]) -> AccountId {
    let domain = address.rsplit_once('@').map_or("", |(_, d)| d);
    let label = crate::profile::slug(domain.split('.').next().unwrap_or(""));
    let base = if label.is_empty() { "mail".to_owned() } else { label };
    let mut id = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.0.eq_ignore_ascii_case(&id)) {
        id = format!("{base}-{n}");
        n += 1;
    }
    AccountId(id)
}

fn closed(flow: &mut Option<Flow>, text: impl Into<String>, error: bool) -> Vec<FlowEffect> {
    *flow = None;
    vec![FlowEffect::Closed { text: text.into(), error }]
}

/// Leave the review for the secret: a password prompt, the browser, or the missing-client-id page.
fn proceed(mut form: Form, providers: Providers) -> (Flow, Vec<FlowEffect>) {
    let config = match form.config() {
        Ok(config) => config,
        Err(problem) => {
            form.error = Some(problem);
            return (Flow::Review { form }, vec![]);
        }
    };
    form.error = None;
    match config.auth {
        AuthKind::Password => (Flow::Password { form, input: Secret::new(), existing: false }, vec![]),
        AuthKind::OAuth2(provider) if !providers.has(provider) => (Flow::NoClientId { form, provider }, vec![]),
        AuthKind::OAuth2(_) => (Flow::SigningIn { form }, vec![FlowEffect::SignIn(config)]),
    }
}

pub fn step(flow: &mut Option<Flow>, event: FlowEvent, providers: Providers) -> Vec<FlowEffect> {
    let Some(state) = flow.take() else { return vec![] };
    let esc = matches!(&event, FlowEvent::Key(k) if k.key == K::Esc);
    let enter = matches!(&event, FlowEvent::Key(k) if k.key == K::Enter);
    let (next, effects) = match (state, event) {
        (Flow::Discovering { .. }, FlowEvent::Key(_)) if esc => return closed(flow, "account not added", false),
        (Flow::Discovering { id, email }, FlowEvent::Discovered(Ok(settings))) => (Flow::Review { form: Form::from_settings(id, &email, settings) }, vec![]),
        (Flow::Discovering { .. }, FlowEvent::Discovered(Err(reason))) => return closed(flow, reason, true),

        (Flow::Review { .. }, FlowEvent::Key(_)) if esc => return closed(flow, "account not added", false),
        (Flow::Review { form }, FlowEvent::Key(_)) if enter => proceed(form, providers),
        (Flow::Review { mut form }, FlowEvent::Key(key)) => {
            form.edit(key);
            (Flow::Review { form }, vec![])
        }

        (Flow::Password { existing: true, .. }, FlowEvent::Key(_)) if esc => return closed(flow, "password not changed", false),
        (Flow::Password { form, .. }, FlowEvent::Key(_)) if esc => (Flow::Review { form }, vec![]),
        (Flow::Password { form, input, existing }, FlowEvent::Key(_)) if enter => {
            if input.is_empty() {
                (Flow::Password { form, input, existing }, vec![])
            } else {
                match form.config() {
                    Ok(config) => (Flow::Saving { form }, vec![FlowEffect::Save { config, password: Some(input) }]),
                    Err(problem) => (Flow::Review { form: Form { error: Some(problem), ..form } }, vec![]),
                }
            }
        }
        (Flow::Password { form, mut input, existing }, FlowEvent::Key(key)) => {
            match key.key {
                K::Backspace => input.pop(),
                _ => if let Some(c) = key.typed() { input.push(c); },
            }
            (Flow::Password { form, input, existing }, vec![])
        }

        (Flow::NoClientId { form, .. }, FlowEvent::Key(_)) if esc => (Flow::Review { form }, vec![]),
        (Flow::NoClientId { mut form, provider }, FlowEvent::Key(_)) if enter && allows_app_password(provider) => {
            form.set_auth(&AuthKind::Password);
            // An app password is filed by the client, like any password account's sent mail.
            (Flow::Password { form, input: Secret::new(), existing: false }, vec![])
        }

        (Flow::SigningIn { form }, FlowEvent::Key(_)) if esc => (Flow::Review { form }, vec![FlowEffect::CancelSignIn]),
        (Flow::SigningIn { form }, FlowEvent::SignedIn(Ok(()))) => match form.config() {
            Ok(config) => (Flow::Saving { form }, vec![FlowEffect::Save { config, password: None }]),
            Err(problem) => (Flow::Review { form: Form { error: Some(problem), ..form } }, vec![]),
        },
        (Flow::SigningIn { form }, FlowEvent::SignedIn(Err(reason))) => (Flow::Review { form: Form { error: Some(reason), ..form } }, vec![]),

        (Flow::Saving { form }, FlowEvent::Saved(Ok(()))) => return closed(flow, format!("account {} saved · connecting", form.id), false),
        (Flow::Saving { form }, FlowEvent::Saved(Err(reason))) => (Flow::Review { form: Form { error: Some(reason), ..form } }, vec![]),

        // Answers that arrive after the flow moved on, and keys a state has no use for.
        (state, _) => (state, vec![]),
    };
    *flow = Some(next);
    effects
}

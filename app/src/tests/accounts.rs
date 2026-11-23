//! Adding and removing accounts: the form's state machine by itself, then the whole path through
//! `AppState` on a temporary profile with in-memory secrets and no network. Accounts point at a
//! closed port of this machine, so connecting fails at once.

use super::profile::{TempProfile, local_account, real_state};
use crate::accounts_ui::{self, FieldKey, Flow, FlowEffect, FlowEvent, Form, Providers, Secret, step};
use crate::keys::{K, KeyPress};
use crate::state::AppState;
use crate::view;
use mail_auth::autoconfig::{self, Source};
use mail_auth::oauth::Provider;
use mail_auth::{AuthKind, MemorySecrets, SecretKind, SecretStore};
use mail_core::AccountId;
use rune::text::TextSystem;
use rune::{Env, Runtime};
use std::sync::Arc;

const PASSWORD: &str = "correct horse battery staple";

fn key(k: K) -> FlowEvent { FlowEvent::Key(KeyPress::named(k)) }

fn type_into(flow: &mut Option<Flow>, text: &str, providers: Providers) {
    for c in text.chars() { assert!(step(flow, FlowEvent::Key(KeyPress::char(c)), providers).is_empty()); }
}

fn discovered(email: &str) -> Option<Flow> {
    let mut flow = Some(Flow::Discovering { id: AccountId::from("norrland"), email: email.into() });
    let settings = autoconfig::discover(email, &|_| None).expect("an address");
    assert!(step(&mut flow, FlowEvent::Discovered(Ok(settings)), Providers::default()).is_empty());
    flow
}

fn press_all(s: &mut AppState, text: &str) {
    for c in text.chars() { s.key(KeyPress::char(c)); }
}

/// Everything the view draws, as text.
fn screen_text(state: AppState) -> (String, AppState) {
    let text = TextSystem::new(crate::FONTS, crate::FAMILIES);
    let mut rt = Runtime::with_text(state, view::root, (1400.0, 880.0), Env { system_dark: true, scale: 2.0 }, text);
    rt.poll = Some(crate::poll);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let all = rt.elements().iter().filter_map(|e| e.text.clone()).collect::<Vec<_>>().join("\n");
    (all, rt.state)
}

#[test]
fn a_secret_is_never_shown_and_is_wiped() {
    let mut secret = Secret::new();
    for c in PASSWORD.chars() { secret.push(c); }
    assert_eq!(secret.expose(), PASSWORD);
    assert_eq!(secret.masked(), "•".repeat(PASSWORD.chars().count()));
    assert!(!format!("{secret:?}").contains("horse"));
    secret.pop();
    assert_eq!(secret.len(), PASSWORD.len() - 1);
    secret.wipe();
    assert!(secret.is_empty());
    for _ in 0..Secret::MAX + 10 { secret.push('x'); }
    assert_eq!(secret.len(), Secret::MAX, "it never grows past its buffer");
}

#[test]
fn a_guessed_host_is_marked_and_can_be_edited_before_the_password_is_asked() {
    let none = Providers::default();
    let mut flow = discovered("huginn@norrland.example");
    let form = flow.as_ref().and_then(Flow::form).expect("the form").clone();
    assert_eq!(form.source, Some(Source::Guess));
    assert_eq!(form.config().expect("valid").incoming, mail_auth::Incoming::Imap(mail_auth::Server::new("imap.norrland.example", 993, mail_auth::Security::Tls)));

    // Down to the IMAP server, rub out the guess, type the real host.
    step(&mut flow, key(K::Down), none);
    step(&mut flow, key(K::Down), none);
    for _ in 0.."imap.norrland.example".len() { step(&mut flow, key(K::Backspace), none); }
    type_into(&mut flow, "mail.norrland.example", none);
    // The port takes digits only; security is a choice the arrow keys change.
    step(&mut flow, key(K::Down), none);
    type_into(&mut flow, "x", none);
    step(&mut flow, key(K::Down), none);
    step(&mut flow, key(K::Right), none);
    let config = flow.as_ref().and_then(Flow::form).expect("the form").config().expect("valid");
    assert_eq!(config.incoming, mail_auth::Incoming::Imap(mail_auth::Server::new("mail.norrland.example", 993, mail_auth::Security::StartTls)));

    assert!(step(&mut flow, key(K::Enter), none).is_empty());
    assert!(matches!(flow, Some(Flow::Password { existing: false, .. })), "{flow:?}");
    type_into(&mut flow, PASSWORD, none);
    assert!(!format!("{flow:?}").contains("horse"), "the password is not in Debug output");
    let effects = step(&mut flow, key(K::Enter), none);
    match effects.as_slice() {
        [FlowEffect::Save { config, password: Some(password) }] => {
            assert_eq!(config.id, AccountId::from("norrland"));
            assert_eq!(password.expose(), PASSWORD);
        }
        other => panic!("expected one save, got {other:?}"),
    }
    assert!(matches!(flow, Some(Flow::Saving { .. })));
    let closed = step(&mut flow, FlowEvent::Saved(Ok(())), none);
    assert!(matches!(closed.as_slice(), [FlowEffect::Closed { error: false, .. }]) && flow.is_none());
}

#[test]
fn a_form_that_does_not_make_an_account_says_why_and_stays() {
    let none = Providers::default();
    let mut flow = discovered("huginn@norrland.example");
    for _ in 0..3 { step(&mut flow, key(K::Down), none); }
    for _ in 0..3 { step(&mut flow, key(K::Backspace), none); }
    assert!(step(&mut flow, key(K::Enter), none).is_empty());
    let form = flow.as_ref().and_then(Flow::form).expect("the form");
    assert!(matches!(flow, Some(Flow::Review { .. })) && form.error.as_deref().is_some_and(|e| e.contains("port")), "{form:?}");

    // Unencrypted connections are refused for anything but this machine.
    let mut flow = discovered("huginn@norrland.example");
    for _ in 0..4 { step(&mut flow, key(K::Down), none); }
    step(&mut flow, key(K::Left), none);
    step(&mut flow, key(K::Enter), none);
    assert!(flow.as_ref().and_then(Flow::form).and_then(|f| f.error.clone()).is_some_and(|e| e.contains("unencrypted")));
    // Esc leaves without saving anything.
    let effects = step(&mut flow, key(K::Esc), none);
    assert!(matches!(effects.as_slice(), [FlowEffect::Closed { .. }]) && flow.is_none());
}

#[test]
fn an_oauth_provider_without_a_client_id_says_so_and_offers_an_app_password_where_allowed() {
    let none = Providers::default();
    let mut flow = discovered("muninn@gmail.com");
    assert_eq!(flow.as_ref().and_then(Flow::form).map(|f| (f.source, f.auth())), Some((Some(Source::Builtin), AuthKind::OAuth2(Provider::Google))));
    assert!(step(&mut flow, key(K::Enter), none).is_empty(), "no sign-in is started");
    assert!(matches!(flow, Some(Flow::NoClientId { provider: Provider::Google, .. })), "{flow:?}");
    step(&mut flow, key(K::Enter), none);
    assert!(matches!(flow, Some(Flow::Password { .. })));
    assert_eq!(flow.as_ref().and_then(Flow::form).map(Form::auth), Some(AuthKind::Password));

    let mut flow = discovered("muninn@outlook.com");
    step(&mut flow, key(K::Enter), none);
    assert!(matches!(flow, Some(Flow::NoClientId { provider: Provider::Microsoft, .. })));
    step(&mut flow, key(K::Enter), none);
    assert!(matches!(flow, Some(Flow::NoClientId { .. })), "Microsoft takes no passwords for IMAP");
    step(&mut flow, key(K::Esc), none);
    assert!(matches!(flow, Some(Flow::Review { .. })));
}

#[test]
fn an_oauth_provider_with_a_client_id_goes_to_the_browser_and_saves_no_password() {
    let google = Providers { google: true, microsoft: false };
    let mut flow = discovered("muninn@gmail.com");
    let effects = step(&mut flow, key(K::Enter), google);
    assert!(matches!(effects.as_slice(), [FlowEffect::SignIn(config)] if config.auth == AuthKind::OAuth2(Provider::Google)), "{effects:?}");
    assert!(matches!(flow, Some(Flow::SigningIn { .. })));
    let effects = step(&mut flow, FlowEvent::SignedIn(Ok(())), google);
    assert!(matches!(effects.as_slice(), [FlowEffect::Save { password: None, .. }]), "{effects:?}");

    // Esc while the browser is open cancels the listener and goes back to the form.
    let mut flow = discovered("muninn@gmail.com");
    step(&mut flow, key(K::Enter), google);
    assert!(matches!(step(&mut flow, key(K::Esc), google).as_slice(), [FlowEffect::CancelSignIn]));
    assert!(matches!(flow, Some(Flow::Review { .. })));
    // A refused sign-in is shown on the form.
    step(&mut flow, key(K::Enter), google);
    step(&mut flow, FlowEvent::SignedIn(Err("the sign-in was cancelled or refused: access_denied".into())), google);
    assert!(flow.as_ref().and_then(Flow::form).and_then(|f| f.error.clone()).is_some_and(|e| e.contains("refused")));
}

#[test]
fn new_account_ids_come_from_the_domain_and_never_repeat() {
    let taken = [AccountId::from("gmail"), AccountId::from("gmail-2")];
    assert_eq!(accounts_ui::new_account_id("a@icloud.com", &taken), AccountId::from("icloud"));
    assert_eq!(accounts_ui::new_account_id("a@gmail.com", &taken), AccountId::from("gmail-3"));
    assert_eq!(accounts_ui::new_account_id("a@localhost", &[]), AccountId::from("localhost"));
    assert!(crate::profile::valid_account_id(&accounts_ui::new_account_id("a@Ünï.example", &[]).0));
}

/// Point the form at a closed local port, so saving the account never reaches the network.
fn make_local(s: &mut AppState) {
    let Some(Flow::Review { form }) = &mut s.shell.flow else { panic!("the form is not up: {:?}", s.shell.flow) };
    for field in &mut form.fields {
        match field.key {
            FieldKey::ImapHost | FieldKey::SmtpHost => field.value = "127.0.0.1".into(),
            FieldKey::ImapPort | FieldKey::SmtpPort => field.value = "1".into(),
            FieldKey::ImapSecurity | FieldKey::SmtpSecurity => field.value = "none (this machine only)".into(),
            _ => {}
        }
    }
}

#[test]
fn an_account_is_added_from_the_command_line_and_its_password_only_reaches_the_secret_store() {
    let temp = TempProfile::new("add");
    let secrets = Arc::new(MemorySecrets::default());
    let mut s = real_state(&temp, secrets.clone());
    assert!(s.shell.onboarding());
    press_all(&mut s, ":account add huginn@norrland.example");
    s.key(KeyPress::named(K::Enter));
    assert!(matches!(s.shell.flow, Some(Flow::Review { .. })), "{:?} {:?}", s.shell.flow, s.toast);
    assert_eq!(s.shell.flow.as_ref().and_then(Flow::form).and_then(|f| f.source), Some(Source::Guess), "nothing was found on the (absent) network");
    make_local(&mut s);
    s.key(KeyPress::named(K::Enter));
    // Keys that are commands elsewhere are only characters of the password here.
    press_all(&mut s, PASSWORD);
    assert!(matches!(s.shell.flow, Some(Flow::Password { .. })));

    let (text, mut s) = screen_text(s);
    assert!(!text.contains("horse") && !text.contains(PASSWORD), "the password is not drawn");
    assert!(text.contains(&"•".repeat(PASSWORD.chars().count())), "it shows as bullets:\n{text}");

    s.key(KeyPress::named(K::Enter));
    assert!(s.shell.flow.is_none(), "{:?}", s.shell.flow);
    assert_eq!(s.shell.accounts.len(), 1);
    assert!(!s.shell.onboarding());
    let id = AccountId::from("norrland");
    assert_eq!(secrets.get(&id, SecretKind::Password).expect("the store"), Some(PASSWORD.to_owned()));
    let file = temp.read("accounts.toml");
    assert!(file.contains("huginn@norrland.example") && file.contains("127.0.0.1"), "{file}");
    for name in ["accounts.toml", "config.toml"] { assert!(!temp.read(name).contains("horse"), "{name} holds the password"); }
    assert!(s.toast.iter().all(|t| !t.text.contains("horse")));
    assert_eq!(s.workspaces.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["norrland"]);
    assert!(s.shell.problems.is_empty(), "it connected (lazily): {:?}", s.shell.problems);
    assert!(s.mail.sync.contains_key(&id), "the engine opened it: {:?}", s.mail.sync);
    assert!(temp.dir.join("mail.db").exists());

    // The same address twice is refused.
    press_all(&mut s, ":account add huginn@norrland.example");
    s.key(KeyPress::named(K::Enter));
    assert!(s.shell.flow.is_none() && s.toast.as_ref().is_some_and(|t| t.error));

    // Removing it takes the settings, the secret and (as the last account) the mail cache.
    press_all(&mut s, ":account remove norrland");
    s.key(KeyPress::named(K::Enter));
    assert!(s.shell.accounts.is_empty() && s.shell.onboarding(), "{:?}", s.toast);
    assert_eq!(secrets.get(&id, SecretKind::Password).expect("the store"), None);
    assert!(!temp.read("accounts.toml").contains("norrland"));
    assert!(!temp.dir.join("mail.db").exists(), "the cache went with the last account");
}

#[test]
fn an_account_without_a_stored_password_is_named_and_does_not_stop_the_others() {
    let temp = TempProfile::new("problems");
    temp.profile().save_accounts(&[local_account("home"), local_account("work")]).expect("saved");
    let secrets = Arc::new(MemorySecrets::default());
    secrets.set(&AccountId::from("work"), SecretKind::Password, PASSWORD).expect("the store");
    let mut s = real_state(&temp, secrets.clone());
    assert_eq!(s.shell.problems.keys().cloned().collect::<Vec<_>>(), [AccountId::from("home")]);
    let line = s.shell.status_line().expect("a status line");
    assert!(line.starts_with("home: no password stored") && line.contains(":account password home"), "{line}");
    assert!(s.mail.sync.contains_key(&AccountId::from("work")), "the other account was opened: {:?}", s.mail.sync);
    assert!(!s.mail.sync.contains_key(&AccountId::from("home")));

    // Typing the missing password stores it and connects the account.
    press_all(&mut s, ":account password home");
    s.key(KeyPress::named(K::Enter));
    assert!(matches!(s.shell.flow, Some(Flow::Password { existing: true, .. })), "{:?}", s.toast);
    press_all(&mut s, PASSWORD);
    s.key(KeyPress::named(K::Enter));
    assert!(s.shell.flow.is_none());
    assert_eq!(secrets.get(&AccountId::from("home"), SecretKind::Password).expect("the store"), Some(PASSWORD.to_owned()));
    assert!(s.shell.problems.is_empty() && s.shell.status_line().is_none(), "{:?}", s.shell.problems);
    assert_eq!(temp.profile().load().accounts.len(), 2, "the settings are kept");
}

#[test]
fn an_oauth_account_without_a_client_id_is_a_clear_state_not_a_crash() {
    let temp = TempProfile::new("oauth");
    let mut gmail = local_account("gmail");
    gmail.incoming = mail_auth::Incoming::Imap(mail_auth::Server::new("imap.gmail.com", 993, mail_auth::Security::Tls));
    gmail.smtp = Some(mail_auth::Server::new("smtp.gmail.com", 465, mail_auth::Security::Tls));
    gmail.auth = AuthKind::OAuth2(Provider::Google);
    temp.profile().save_accounts(&[gmail]).expect("saved");
    let mut s = real_state(&temp, Arc::new(MemorySecrets::default()));
    assert!(s.shell.status_line().is_some_and(|l| l.contains("no Google OAuth client id")), "{:?}", s.shell.status_line());
    press_all(&mut s, ":account signin gmail");
    s.key(KeyPress::named(K::Enter));
    assert!(s.shell.flow.is_none() && s.toast.as_ref().is_some_and(|t| t.error && t.text.contains("RAVENGLASS_GOOGLE_CLIENT_ID")), "{:?}", s.toast);
}

#[test]
fn the_demo_keeps_its_accounts_and_takes_no_new_ones() {
    let mut s = AppState::demo_inline();
    press_all(&mut s, ":account add huginn@norrland.example");
    s.key(KeyPress::named(K::Enter));
    assert!(s.shell.flow.is_none() && s.toast.as_ref().is_some_and(|t| t.error));
    assert!(!s.shell.onboarding() && s.shell.status_line().is_none());
}

#[test]
fn the_onboarding_pane_says_how_to_add_an_account_and_shows_the_command_line() {
    let temp = TempProfile::new("onboarding");
    let mut s = real_state(&temp, Arc::new(MemorySecrets::default()));
    press_all(&mut s, ":account add h");
    let (text, s) = screen_text(s);
    assert!(text.contains(view::onboarding::CAPTION) && text.contains(":account add you@example.com"), "{text}");
    assert!(text.contains("account add h"), "the command being typed shows:\n{text}");
    assert!(text.contains("no accounts"), "the workspace bar says so:\n{text}");
    assert!(s.link.is_off(), "no engine without accounts");
}

#[test]
fn the_form_scene_draws_the_guess_warning_and_every_field() {
    let mut s = AppState::demo_inline();
    let (_, scene) = crate::scenes::SCENES.iter().find(|(name, _)| *name == "account-form").expect("the scene");
    scene(&mut s);
    let (text, _) = screen_text(s);
    assert!(text.contains("guessed from the domain"), "{text}");
    for label in ["imap server", "smtp port", "sign-in"] { assert!(text.contains(label), "{label} is missing:\n{text}"); }
}

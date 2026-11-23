//! The app lock: while it is on, the view tree (what `--tree` reports) holds the wallpaper and
//! the lock line and nothing with message content; keys reach nothing but the lock; and only the
//! right password, checked through the secret store, opens it.

use super::profile::{TempProfile, local_account, real_state};
use crate::actions::Action;
use crate::keys::{K, KeyPress};
use crate::platform;
use crate::shell::ShellCmd;
use crate::state::AppState;
use crate::view;
use mail_auth::{MemorySecrets, SecretKind, SecretStore};
use mail_core::AccountId;
use rune::runtime::ElementInfo;
use rune::text::TextSystem;
use rune::{Env, Key, KeyEvent, Mods, Runtime};
use std::sync::Arc;

const FRAME: f64 = 1.0 / 60.0;
const PASSWORD: &str = "open sesame 42";

fn runtime(state: AppState) -> Runtime<AppState> {
    let text = TextSystem::new(crate::FONTS, crate::FAMILIES);
    let mut rt = Runtime::with_text(state, view::root, (1400.0, 880.0), Env { system_dark: true, scale: 2.0 }, text);
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt.frame(FRAME);
    rt
}

fn press(rt: &mut Runtime<AppState>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(FRAME);
}

fn type_keys(rt: &mut Runtime<AppState>, text: &str) {
    for c in text.chars() { press(rt, Key::Text(c.to_string())); }
}

/// `:lock`, typed as a user would.
fn lock(rt: &mut Runtime<AppState>) {
    type_keys(rt, ":lock");
    press(rt, Key::Enter);
    assert!(rt.state.shell.lock.concealed());
}

fn texts(els: &[ElementInfo]) -> String { els.iter().filter_map(|e| e.text.clone()).collect::<Vec<_>>().join("\n") }

/// The triage view of the demo: a list, an open message, tabs.
fn triage() -> AppState {
    let mut s = AppState::demo_inline();
    s.open_selected(false);
    s
}

/// A profile with one password account whose password is in the secret store.
fn with_password(temp: &TempProfile, config: &str) -> AppState {
    temp.profile().save_accounts(&[local_account("home")]).expect("saved");
    temp.write("config.toml", config);
    let secrets = Arc::new(MemorySecrets::default());
    secrets.set(&AccountId::from("home"), SecretKind::Password, PASSWORD).expect("the store");
    real_state(temp, secrets)
}

#[test]
fn the_lock_screen_holds_no_list_no_reader_and_no_subject() {
    let mut rt = runtime(triage());
    let subject = rt.state.ws().open.as_ref().expect("an open message").subject.clone();
    let sender = rt.state.selected().expect("a row").from.email.clone();
    let before = rt.elements();
    assert!(before.iter().any(|e| e.role == "listitem") && texts(&before).contains(&subject), "the triage view shows mail");

    lock(&mut rt);
    let els = rt.elements();
    let all = texts(&els);
    assert!(all.contains("Locked · press any key"), "{all}");
    assert_eq!(els.iter().filter(|e| e.role == "listitem" || e.role == "list" || e.role == "tab" || e.role == "tablist").count(), 0, "no list rows and no tabs");
    for key in ["list:1", "reader:2", "tabs", "workspaces", "status", "toast"] {
        assert!(!els.iter().any(|e| e.key.as_deref() == Some(key)), "{key} is still in the tree");
    }
    assert!(!all.contains(&subject) && !all.contains(&sender), "message content is on the lock screen:\n{all}");
    for word in ["Inbox", "iCloud", "Gmail", "archived"] { assert!(!all.contains(word), "{word} is on the lock screen:\n{all}"); }
    // Only the wallpaper's words are left.
    let words: Vec<&str> = all.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    assert_eq!(words, ["R A V E N G L A S S", "Locked · press any key"]);
}

#[test]
fn locking_from_the_command_line_and_any_key_lifts_a_lock_with_no_password_account() {
    let mut rt = runtime(triage());
    type_keys(&mut rt, ":lock");
    press(&mut rt, Key::Enter);
    assert!(rt.state.shell.lock.concealed());
    assert!(rt.state.toast.is_none() && rt.state.mode == crate::modes::Mode::default());
    // The demo has no password to ask for: the lock is a curtain.
    press(&mut rt, Key::Text("j".into()));
    assert!(!rt.state.shell.lock.concealed());
    assert!(rt.elements().iter().any(|e| e.role == "listitem"), "the list is back");
    assert_eq!(rt.state.ws().sel, 0, "the key that lifted the lock did nothing else");
}

#[test]
fn keys_reach_nothing_while_locked_and_only_the_right_password_unlocks() {
    let temp = TempProfile::new("lock-password");
    let mut rt = runtime(with_password(&temp, ""));
    lock(&mut rt);
    assert!(texts(&rt.elements()).contains("Locked · press any key"));

    // Any key asks for the password; it is not part of it.
    press(&mut rt, Key::Text(":".into()));
    assert!(rt.state.shell.lock.prompting() && rt.state.shell.lock.input.is_empty());
    assert_eq!(rt.state.mode, crate::modes::Mode::default(), "the key did not open the command line");
    type_keys(&mut rt, "not it");
    let shown = texts(&rt.elements());
    assert!(shown.contains("home password") && shown.contains("••••••") && !shown.contains("not it"), "{shown}");
    press(&mut rt, Key::Enter);
    assert!(rt.state.shell.lock.concealed() && rt.state.shell.lock.refused, "a wrong password stays locked");
    assert!(texts(&rt.elements()).contains("that was not the password"));

    type_keys(&mut rt, PASSWORD);
    assert!(!texts(&rt.elements()).contains("sesame"));
    press(&mut rt, Key::Enter);
    assert!(!rt.state.shell.lock.concealed(), "the right one unlocks");
    assert!(rt.state.shell.lock.input.is_empty(), "nothing typed is kept");
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("workspaces")));
}

#[test]
fn escape_on_the_password_line_goes_back_to_locked() {
    let temp = TempProfile::new("lock-esc");
    let mut s = with_password(&temp, "");
    s.perform(Action::Shell(ShellCmd::Lock));
    s.key(KeyPress::char('x'));
    s.key(KeyPress::char('y'));
    assert!(s.shell.lock.prompting() && s.shell.lock.input.len() == 1);
    s.key(KeyPress::named(K::Esc));
    assert!(s.shell.lock.concealed() && !s.shell.lock.prompting() && s.shell.lock.input.is_empty());
}

#[test]
fn the_app_locks_after_the_idle_time_and_activity_pushes_it_back() {
    let temp = TempProfile::new("lock-idle");
    let mut s = with_password(&temp, "[privacy]\nlock_after_minutes = 1\n");
    assert!(!s.shell.lock.concealed());
    s.shell_poll(1.0);
    assert!(s.fx.contains(&crate::state::Fx::WakeAt(60.0)), "the window is asked to wake for the timer: {:?}", s.fx);
    s.clock = 50.0;
    s.key(KeyPress::char('j'));
    assert!(!s.shell_poll(61.0), "a key at 50 s keeps it open past 60 s");
    assert!(!s.shell.lock.concealed());
    assert!(s.shell_poll(111.0), "and it locks a minute after that key");
    assert!(s.shell.lock.concealed());
}

#[test]
fn lock_on_launch_starts_concealed_and_asks_for_the_password() {
    let temp = TempProfile::new("lock-launch");
    let rt = runtime(with_password(&temp, "[privacy]\nlock_on_launch = true\n"));
    assert!(rt.state.shell.lock.concealed() && rt.state.shell.lock.prompting());
    let els = rt.elements();
    assert!(!els.iter().any(|e| e.role == "listitem" || e.role == "tab"), "nothing but the lock at launch");
    assert!(texts(&els).contains("home password"));
}

#[test]
fn the_portable_password_check_goes_through_the_secret_store() {
    let secrets = MemorySecrets::default();
    let id = AccountId::from("home");
    assert!(!platform::check_password(&secrets, &id, ""), "nothing stored, nothing matches");
    secrets.set(&id, SecretKind::Password, PASSWORD).expect("the store");
    assert!(platform::check_password(&secrets, &id, PASSWORD));
    for wrong in ["", "open sesame 4", "open sesame 421", "OPEN SESAME 42"] { assert!(!platform::check_password(&secrets, &id, wrong), "{wrong:?}"); }
    assert!(!platform::check_password(&secrets, &AccountId::from("work"), PASSWORD));
    assert!(platform::constant_time_eq(b"", b"") && !platform::constant_time_eq(b"a", b"ab"));
}

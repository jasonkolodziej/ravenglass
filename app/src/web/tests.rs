//! The Relay sign-in machine, on every target.

use super::session::*;
use crate::keys::{K, KeyPress};
use mail_core::AccountId;

fn typed(state: &mut Session, text: &str) -> Vec<Effect> {
    text.chars().flat_map(|c| step(state, Event::Key(KeyPress::char(c)))).collect()
}

fn press(state: &mut Session, key: K) -> Vec<Effect> { step(state, Event::Key(KeyPress::named(key))) }

fn signed(grant: Grant) -> Signed { Signed { grant, who: "odin".into(), accounts: vec![AccountId::from("iCloud")] } }

fn signin() -> Session {
    let mut s = Session::default();
    assert!(step(&mut s, Event::Checked(Err(Problem::Refused))).is_empty());
    s
}

#[test]
fn a_session_found_at_start_goes_straight_to_the_mail() {
    let mut s = Session::default();
    let ok = signed(Grant::Cookie { csrf: "c".into() });
    assert_eq!(step(&mut s, Event::Checked(Ok(ok.clone()))), vec![Effect::Start(ok)]);
    assert!(s.is_ready());
}

#[test]
fn no_session_shows_the_form_without_an_error_but_an_unreachable_relay_says_so() {
    let s = signin();
    assert!(s.form().is_some_and(|f| f.error.is_none()));
    let mut s = Session::default();
    step(&mut s, Event::Checked(Err(Problem::Offline("Failed to fetch".into()))));
    assert!(s.form().and_then(|f| f.error.as_deref()).is_some_and(|e| e.starts_with("the relay cannot be reached")));
}

#[test]
fn username_and_password_sign_in_and_the_password_is_masked_and_wiped() {
    let mut s = signin();
    typed(&mut s, "odin");
    press(&mut s, K::Tab);
    typed(&mut s, "hunter2");
    assert_eq!(s.form().map(|f| f.password.masked()), Some("•••••••".into()));
    let effects = press(&mut s, K::Enter);
    assert_eq!(effects, vec![Effect::Login { user: "odin".into(), password: "hunter2".into() }]);
    assert!(matches!(s, Session::SigningIn(_)));
    // Keys wait while the answer is on its way.
    assert!(typed(&mut s, "x").is_empty());
    let ok = signed(Grant::Cookie { csrf: "c".into() });
    assert_eq!(step(&mut s, Event::Answered(Ok(ok.clone()))), vec![Effect::Start(ok)]);
    assert!(s.is_ready());
}

#[test]
fn a_wrong_password_is_said_and_the_password_must_be_typed_again() {
    let mut s = signin();
    typed(&mut s, "odin");
    press(&mut s, K::Down);
    typed(&mut s, "nope");
    press(&mut s, K::Enter);
    step(&mut s, Event::Answered(Err(Problem::Refused)));
    let form = s.form().expect("the form is back");
    assert_eq!(form.error.as_deref(), Some("wrong username or password"));
    assert!(form.password.is_empty());
    assert_eq!(form.user, "odin");
}

#[test]
fn the_token_signs_in_on_its_own_and_is_kept_for_the_tab() {
    let mut s = signin();
    press(&mut s, K::Tab);
    press(&mut s, K::Tab);
    typed(&mut s, " relay-token-0123456789 ");
    assert_eq!(press(&mut s, K::Enter), vec![Effect::UseToken("relay-token-0123456789".into())]);
    let ok = signed(Grant::Token("relay-token-0123456789".into()));
    assert_eq!(step(&mut s, Event::Answered(Ok(ok.clone()))), vec![Effect::KeepToken("relay-token-0123456789".into()), Effect::Start(ok)]);
}

#[test]
fn missing_fields_are_named_and_nothing_is_sent() {
    let mut s = signin();
    assert!(press(&mut s, K::Enter).is_empty());
    assert_eq!(s.form().and_then(|f| f.error.as_deref()), Some("type your username"));
    typed(&mut s, "odin");
    assert!(press(&mut s, K::Enter).is_empty());
    assert_eq!(s.form().map(|f| f.field), Some(Field::Password));
    press(&mut s, K::Up);
    press(&mut s, K::Up);
    assert!(press(&mut s, K::Enter).is_empty());
    assert_eq!(s.form().and_then(|f| f.error.as_deref()), Some("type the relay's token"));
}

#[test]
fn a_refused_token_says_so_and_signing_out_forgets_it() {
    let mut s = signin();
    press(&mut s, K::Up);
    typed(&mut s, "wrong");
    press(&mut s, K::Enter);
    step(&mut s, Event::Answered(Err(Problem::Refused)));
    assert_eq!(s.form().and_then(|f| f.error.as_deref()), Some("the relay did not take that token"));
    let mut s = Session::Ready(signed(Grant::Token("t".into())));
    assert_eq!(step(&mut s, Event::SignedOut), vec![Effect::Forget]);
    assert!(s.form().is_some());
}

#[test]
fn grants_never_print_their_secret() {
    assert_eq!(format!("{:?}", Grant::Token("relay-token-0123456789".into())), "Grant::Token");
    assert_eq!(format!("{:?}", Grant::Cookie { csrf: "csrf-value".into() }), "Grant::Cookie");
}

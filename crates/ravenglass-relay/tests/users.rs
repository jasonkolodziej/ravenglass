//! Users mode over real HTTP on loopback: sign-in, cookies and CSRF, per-user accounts and
//! their isolation, session expiry, rate limiting and encrypted storage.

mod common;

use common::*;
use mail_core::*;
use ravenglass_engine::{Command, Config, Engine, Event};
use relay_client::{HttpTransport, RelayBackend};
use std::sync::Arc;

#[test]
fn a_signed_in_engine_runs_over_the_relay_with_its_own_account() {
    let relay = start(|_, _| {});
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    assert_eq!(alice.add_account(&relay.base, "work", "imap-password-1").status, 200);

    let (transport, session) = HttpTransport::login(&relay.base, "alice", ALICE_PW).unwrap();
    assert_eq!(session.user.name, "alice");
    let account = AccountId::from("work");
    let mut engine = Engine::new(mail_store::Store::open_in_memory().unwrap(), Config { undo_send_secs: 0.0 });
    engine.add_backend(Arc::new(RelayBackend::new(account.clone(), transport)));
    let mut run = |cmd| block_on(engine.handle(cmd, 0.0));
    let opened = run(Command::OpenAccount(account.clone()));
    assert!(opened.contains(&Event::SyncState { account: account.clone(), state: mail_core::machine::account_sync::State::Idle }), "{opened:?}");
    let Some(Event::Envelopes { envelopes, .. }) = run(Command::LoadRange { folder: FolderId::from("work/Inbox"), offset: 0, limit: 5 }).pop() else { panic!() };
    let first = envelopes[0].clone();
    let archived = run(Command::MoveToRole { ids: vec![first.id.clone()], role: FolderRole::Archive });
    assert!(archived.iter().any(|e| matches!(e, Event::Applied { sent: true, .. })), "a POST with the CSRF token went through: {archived:?}");
    assert_eq!(*relay.opened.lock().unwrap(), [("work".to_owned(), Some("imap-password-1".to_owned()))], "opened once, with the decrypted secret");
}

#[test]
fn login_sets_a_strict_cookie_and_returns_a_csrf_token() {
    let relay = start(|_, _| {});
    let answer = login(&relay.base, "alice", ALICE_PW);
    assert_eq!(answer.status, 200);
    let cookie = answer.header("set-cookie").unwrap();
    assert!(cookie.starts_with("__Host-ravenglass-session="), "{cookie}");
    for attribute in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/"] { assert!(cookie.split("; ").any(|a| a == attribute), "{attribute} missing from {cookie}"); }
    let value = cookie.split(';').next().unwrap().split_once('=').unwrap().1;
    assert_eq!(value.len(), 43, "32 random bytes, base64url");
    let json = answer.json();
    assert_eq!(json["user"]["name"], "alice");
    assert_eq!(json["csrf"].as_str().unwrap().len(), 43);

    let session = sign_in(&relay.base, "alice", ALICE_PW);
    let again = session.get(&relay.base, "/v1/auth/session").json();
    assert_eq!(again["csrf"].as_str(), Some(session.csrf.as_str()));
    assert_eq!(again["user"]["name"], "alice");
}

#[test]
fn requests_without_the_cookie_or_the_csrf_token_are_refused() {
    let relay = start(|_, _| {});
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    let base = &relay.base;
    assert_eq!(alice.add_account(base, "work", "pw").status, 200);
    assert_eq!(call("GET", &format!("{base}/v1/me/accounts"), &[], None).status, 401);
    assert_eq!(call("POST", &format!("{base}/v1/session"), &[("ravenglass-relay-version", "1")], Some(b"")).status, 401);
    assert_eq!(call("GET", &format!("{base}/v1/auth/session"), &[("cookie", "__Host-ravenglass-session=forged")], None).status, 401);
    assert_eq!(call("GET", &format!("{base}/v1/accounts/work/folders"), &[("authorization", "Bearer anything-at-all-here"), ("ravenglass-relay-version", "1")], None).status, 401, "no bearer tokens in users mode");

    let no_csrf = |path: &str, csrf: Option<&str>| {
        let mut headers = vec![("cookie", alice.cookie.as_str()), ("ravenglass-relay-version", "1"), ("content-type", "application/json")];
        if let Some(csrf) = csrf { headers.push(("x-ravenglass-csrf", csrf)); }
        call("POST", &format!("{base}{path}"), &headers, Some(b"{\"since\":\"\"}")).status
    };
    assert_eq!(no_csrf("/v1/session", None), 403);
    assert_eq!(no_csrf("/v1/accounts/work/folders/work%2FInbox/sync", None), 403);
    assert_eq!(no_csrf("/v1/accounts/work/folders/work%2FInbox/sync", Some("wrong-token")), 403);
    assert_eq!(no_csrf("/v1/accounts/work/folders/work%2FInbox/sync", Some(&alice.csrf)), 200);
    assert_eq!(call("DELETE", &format!("{base}/v1/me/accounts/work"), &[("cookie", &alice.cookie)], None).status, 403);
    assert_eq!(call("POST", &format!("{base}/v1/auth/logout"), &[("cookie", &alice.cookie)], None).status, 403);

    let foreign = call("GET", &format!("{base}/v1/me/accounts"), &[("cookie", &alice.cookie), ("origin", "https://evil.example")], None);
    assert_eq!(foreign.status, 403, "another site's page is refused even with the cookie");
    assert!(foreign.header("access-control-allow-origin").is_none());
    assert_eq!(alice.get(base, "/v1/accounts/work/folders").status, 200);
}

#[test]
fn logout_ends_the_session_and_clears_the_cookie() {
    let relay = start(|_, _| {});
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    let out = call("POST", &format!("{}/v1/auth/logout", relay.base), &[("cookie", &alice.cookie), ("x-ravenglass-csrf", &alice.csrf)], None);
    assert_eq!(out.status, 204);
    let cleared = out.header("set-cookie").unwrap();
    assert!(cleared.starts_with("__Host-ravenglass-session=;") && cleared.contains("Max-Age=0"), "{cleared}");
    assert_eq!(alice.get(&relay.base, "/v1/auth/session").status, 401);
    assert_eq!(alice.get(&relay.base, "/v1/me/accounts").status, 401);
}

#[test]
fn sessions_expire_after_thirty_idle_minutes_and_twelve_hours() {
    let relay = start(|_, _| {});
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    relay.advance(29 * 60);
    assert_eq!(alice.get(&relay.base, "/v1/auth/session").status, 200);
    relay.advance(30 * 60);
    assert_eq!(alice.get(&relay.base, "/v1/auth/session").status, 401, "idle too long");

    let bob = sign_in(&relay.base, "bob", BOB_PW);
    for _ in 0..(12 * 60 / 20 - 1) {
        relay.advance(20 * 60);
        assert_eq!(bob.get(&relay.base, "/v1/auth/session").status, 200);
    }
    relay.advance(20 * 60);
    assert_eq!(bob.get(&relay.base, "/v1/auth/session").status, 401, "twelve hours is the limit however busy");
}

#[test]
fn wrong_passwords_get_one_generic_answer_and_then_back_off() {
    let relay = start(|_, _| {});
    let wrong = login(&relay.base, "alice", "not her password at all");
    let unknown = login(&relay.base, "mallory", "not her password at all");
    assert_eq!((wrong.status, unknown.status), (401, 401));
    assert_eq!(wrong.body, unknown.body, "no hint whether the user exists");
    assert!(wrong.header("set-cookie").is_none());
    for _ in 0..4 { assert_eq!(login(&relay.base, "alice", "still wrong, still wrong").status, 401); }
    let limited = login(&relay.base, "alice", ALICE_PW);
    assert_eq!(limited.status, 429, "even the right password waits");
    assert_eq!(limited.header("retry-after"), Some("1"));
    assert_eq!(login(&relay.base, "bob", BOB_PW).status, 200, "other users are not locked out");
    relay.advance(1);
    assert_eq!(login(&relay.base, "alice", ALICE_PW).status, 200);
    assert_eq!(login(&relay.base, "ALICE", "wrong wrong wrong").status, 401);
}

#[test]
fn one_address_trying_many_names_is_slowed_down() {
    let relay = start(|_, _| {});
    for i in 0..20 { assert_eq!(login(&relay.base, &format!("user{i}"), "guess guess guess").status, 401); }
    assert_eq!(login(&relay.base, "alice", ALICE_PW).status, 429);
}

#[test]
fn login_accepts_only_json_from_the_app_origin() {
    let relay = start(|_, _| {});
    let body = format!("{{\"username\":\"alice\",\"password\":\"{ALICE_PW}\"}}");
    let url = format!("{}/v1/auth/login", relay.base);
    assert_eq!(call("POST", &url, &[("content-type", "text/plain")], Some(body.as_bytes())).status, 415);
    assert_eq!(call("POST", &url, &[("content-type", "application/json"), ("origin", "https://evil.example")], Some(body.as_bytes())).status, 403);
    let ok = call("POST", &url, &[("content-type", "application/json"), ("origin", ORIGIN)], Some(body.as_bytes()));
    assert_eq!(ok.status, 200);
    assert_eq!(ok.header("access-control-allow-origin"), Some(ORIGIN));
    assert_eq!(ok.header("access-control-allow-credentials"), Some("true"));
}

#[test]
fn auth_config_tells_the_client_which_sign_in_to_show() {
    let relay = start(|_, _| {});
    let answer = call("GET", &format!("{}/v1/auth/config", relay.base), &[], None);
    assert_eq!(answer.json(), serde_json::json!({ "mode": "users", "password": true, "oidc": false }));
}

#[test]
fn a_removed_user_loses_access_at_once() {
    let relay = start(|_, _| {});
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    relay.store.remove_user("alice").unwrap();
    assert_eq!(alice.get(&relay.base, "/v1/auth/session").status, 401);
}

#[test]
fn every_response_carries_the_security_headers() {
    let relay = start(|_, _| {});
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    for answer in [alice.get(&relay.base, "/v1/me/accounts"), call("GET", &format!("{}/v1/nope", relay.base), &[], None), login(&relay.base, "x", "y")] {
        assert!(answer.header("content-security-policy").unwrap().starts_with("default-src 'none'"));
        assert_eq!(answer.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(answer.header("referrer-policy"), Some("no-referrer"));
    }
}

#[test]
fn preflights_allow_credentials_for_the_app_origin_only() {
    let relay = start(|_, _| {});
    let preflight = |origin: &str| {
        let agent = agent();
        let response = agent.run(ureq::http::Request::builder().method("OPTIONS").uri(format!("{}/v1/me/accounts", relay.base)).header("origin", origin).header("access-control-request-method", "DELETE").body(()).unwrap()).unwrap();
        let get = |n: &str| response.headers().get(n).map(|v| v.to_str().unwrap().to_owned());
        (response.status().as_u16(), get("access-control-allow-origin"), get("access-control-allow-credentials"), get("access-control-allow-headers"))
    };
    let (status, origin, credentials, headers) = preflight(ORIGIN);
    assert_eq!((status, origin.as_deref(), credentials.as_deref()), (204, Some(ORIGIN), Some("true")));
    assert!(headers.unwrap().contains("x-ravenglass-csrf"));
    assert_eq!(preflight("https://evil.example").1, None);
}

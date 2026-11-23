//! Users mode: nobody can see, use or hear about anyone else's accounts, and nothing readable
//! is left on disk.

mod common;

use common::*;
use futures_util::StreamExt;
use ravenglass_relay::crypto::MasterKey;
use ravenglass_relay::store::Store;
use std::sync::Arc;

/// Every mail route, for an account id, as (method, path, body).
fn routes(account: &str) -> Vec<(&'static str, String, Vec<u8>)> {
    let a = relay_proto::segment(account);
    let inbox = relay_proto::segment(&format!("{account}/Inbox"));
    let message = relay_proto::segment(&format!("{account}/00000"));
    vec![
        ("GET", format!("/v1/accounts/{a}/folders"), vec![]),
        ("POST", format!("/v1/accounts/{a}/folders/{inbox}/sync"), br#"{"since":""}"#.to_vec()),
        ("GET", format!("/v1/accounts/{a}/messages/{message}/raw"), vec![]),
        ("POST", format!("/v1/accounts/{a}/ops"), format!(r#"{{"ops":[{{"Move":{{"ids":["{account}/00000"],"to":"{account}/Archive"}}}}]}}"#).into_bytes()),
        ("POST", format!("/v1/accounts/{a}/send"), br#"{"raw":[104,105],"from":{"name":null,"email":"x@example.net"},"rcpt":[]}"#.to_vec()),
        ("POST", format!("/v1/accounts/{a}/drafts"), b"draft".to_vec()),
    ]
}

fn try_route(session: &Session, base: &str, (method, path, body): &(&str, String, Vec<u8>)) -> Answer {
    if *method == "GET" { session.get(base, path) } else { session.post(base, path, body) }
}

#[test]
fn a_user_cannot_reach_another_users_account_by_any_route() {
    let relay = start(|_, _| {});
    let base = &relay.base;
    let (alice, bob) = (sign_in(base, "alice", ALICE_PW), sign_in(base, "bob", BOB_PW));
    assert_eq!(alice.add_account(base, "alice-private", "alice-imap-pw").status, 200);
    assert_eq!(bob.add_account(base, "bob-own", "bob-imap-pw").status, 200);

    for route in routes("alice-private") {
        let answer = try_route(&bob, base, &route);
        assert_eq!(answer.status, 404, "{} {} must look like no account at all: {}", route.0, route.1, answer.text());
        assert!(!answer.text().contains("alice"));
    }
    assert_eq!(bob.delete(base, "/v1/me/accounts/alice-private").status, 404);
    let bobs: serde_json::Value = bob.get(base, "/v1/me/accounts").json();
    assert_eq!(bobs.as_array().unwrap().len(), 1);
    assert_eq!(bobs[0]["id"], "bob-own");
    let session = bob.post(base, "/v1/session", b"").json();
    assert_eq!(session["accounts"], serde_json::json!(["bob-own"]));
    assert!(relay.opened.lock().unwrap().iter().all(|(id, _)| id != "alice-private"), "bob's requests never opened alice's account");

    for route in routes("alice-private") { assert_eq!(try_route(&alice, base, &route).status, 200, "{} {}", route.0, route.1); }
    let opened = relay.opened.lock().unwrap().clone();
    assert_eq!(opened, [("alice-private".to_owned(), Some("alice-imap-pw".to_owned()))]);
}

#[test]
fn the_same_account_id_for_two_users_is_two_separate_accounts() {
    let relay = start(|_, _| {});
    let base = &relay.base;
    let (alice, bob) = (sign_in(base, "alice", ALICE_PW), sign_in(base, "bob", BOB_PW));
    assert_eq!(alice.add_account(base, "work", "alice-secret").status, 200);
    assert_eq!(alice.add_account(base, "work", "again").status, 409);
    assert_eq!(bob.add_account(base, "work", "bob-secret").status, 200);
    let move_first = br#"{"ops":[{"Move":{"ids":["work/00000"],"to":"work/Archive"}}]}"#;
    assert_eq!(alice.post(base, "/v1/accounts/work/ops", move_first).status, 200);
    let bobs_inbox = bob.post(base, "/v1/accounts/work/folders/work%2FInbox/sync", br#"{"since":""}"#).json();
    assert!(bobs_inbox["added"].as_array().unwrap().iter().any(|e| e["id"] == "work/00000"), "alice's move did not touch bob's mailbox");
    let mut opened = relay.opened.lock().unwrap().clone();
    opened.sort();
    assert_eq!(opened, [("work".to_owned(), Some("alice-secret".to_owned())), ("work".to_owned(), Some("bob-secret".to_owned()))]);
    assert_eq!(bob.delete(base, "/v1/me/accounts/work").status, 204);
    assert_eq!(bob.get(base, "/v1/accounts/work/folders").status, 404);
    assert_eq!(alice.get(base, "/v1/accounts/work/folders").status, 200, "bob removing his account left alice's alone");
}

#[test]
fn unusable_account_settings_are_refused() {
    let relay = start(|_, _| {});
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    let mut plain = account("plain");
    plain.incoming = mail_auth::Incoming::Imap(mail_auth::Server::new("imap.example.net", 143, mail_auth::Security::Plain));
    let body = serde_json::json!({ "config": plain, "secret": { "kind": "Password", "value": "x" } }).to_string();
    assert_eq!(alice.post(&relay.base, "/v1/me/accounts", body.as_bytes()).status, 400);
    assert_eq!(alice.post(&relay.base, "/v1/me/accounts", b"{\"config\":1}").status, 400);
    assert_eq!(alice.get(&relay.base, "/v1/me/accounts").json(), serde_json::json!([]));
}

#[test]
fn secrets_sent_over_http_are_stored_encrypted() {
    let dir = std::env::temp_dir().join(format!("rg-relay-it-{}", ravenglass_relay::crypto::random_token(6)));
    let (path, key_path) = (dir.join("relay.sqlite3"), dir.join("master.key"));
    let (master, created) = MasterKey::load_or_create(&key_path).unwrap();
    assert!(created);
    let relay = start_with_store(Arc::new(Store::open(&path, master).unwrap()), |_, _| {});
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    assert_eq!(alice.add_account(&relay.base, "secret-account-id", "Imap-Pa55word-Plaintext").status, 200);
    let listed = alice.get(&relay.base, "/v1/me/accounts").text();
    assert!(!listed.contains("Imap-Pa55word"), "the secret never comes back");
    let bytes = std::fs::read(&path).unwrap();
    for needle in ["Imap-Pa55word-Plaintext", "secret-account-id@example.net", "secret-account-id", "imap.example.net", ALICE_PW] {
        assert!(!bytes.windows(needle.len()).any(|w| w == needle.as_bytes()), "{needle} is readable in the database");
    }
    let key = std::fs::read(&key_path).unwrap();
    assert_eq!(key.len(), 32);
    assert!(!bytes.windows(32).any(|w| w == key.as_slice()), "the master key is not in the database");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn each_push_socket_hears_only_its_own_users_accounts() {
    let relay = start(|_, _| {});
    let base = &relay.base;
    let (alice, bob) = (sign_in(base, "alice", ALICE_PW), sign_in(base, "bob", BOB_PW));
    assert_eq!(alice.add_account(base, "alice-acct", "a").status, 200);
    assert_eq!(bob.add_account(base, "bob-acct", "b").status, 200);
    let host = base.trim_start_matches("http://");
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let (heard_by_alice, heard_by_bob) = runtime.block_on(async {
        let alice_pushes = listen(host, &alice.cookie);
        let bob_pushes = listen(host, &bob.cookie);
        tokio::join!(alice_pushes, bob_pushes)
    });
    assert!(!heard_by_alice.is_empty() && !heard_by_bob.is_empty(), "{heard_by_alice:?} {heard_by_bob:?}");
    assert!(heard_by_alice.iter().all(|p| p.contains("alice-acct/Inbox")), "{heard_by_alice:?}");
    assert!(heard_by_bob.iter().all(|p| p.contains("bob-acct/Inbox")), "{heard_by_bob:?}");

    let refused = runtime.block_on(tokio_tungstenite::connect_async(format!("ws://{host}/v1/events")));
    assert!(refused.is_err(), "no socket without a session");
    let foreign = runtime.block_on(connect(host, &alice.cookie, "https://evil.example"));
    assert!(foreign.is_err(), "no socket from another site's page");
}

type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(host: &str, cookie: &str, origin: &str) -> Result<WsStream, tokio_tungstenite::tungstenite::Error> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://{host}/v1/events").into_client_request().unwrap();
    request.headers_mut().insert("cookie", cookie.parse().unwrap());
    request.headers_mut().insert("origin", origin.parse().unwrap());
    Ok(tokio_tungstenite::connect_async(request).await?.0)
}

/// Collect pushes for three seconds.
async fn listen(host: &str, cookie: &str) -> Vec<String> {
    let mut socket = connect(host, cookie, ORIGIN).await.unwrap();
    let mut heard = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while let Ok(Some(Ok(message))) = tokio::time::timeout_at(deadline, socket.next()).await {
        if let tokio_tungstenite::tungstenite::Message::Text(text) = message { heard.push(text.to_string()); }
    }
    heard
}

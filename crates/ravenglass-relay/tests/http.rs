//! The Relay over real HTTP on a loopback port: the Relay client, then a whole engine, talk to
//! a mock account through it.

use mail_core::*;
use mail_mock::MockBackend;
use ravenglass_engine::{Command, Config, Engine, Event};
use ravenglass_relay::{Relay, server};
use relay_client::{HttpTransport, RelayBackend};
use std::sync::Arc;

const TOKEN: &str = "correct horse battery staple";
const ORIGIN: &str = "https://mail.example.org";

/// Start a Relay serving one mock account; returns its base URL and the account behind it.
fn start() -> (String, Arc<MockBackend>) {
    let mock = Arc::new(MockBackend::new("iCloud", 46, 40));
    let mut relay = Relay::new();
    relay.add_backend(mock.clone());
    let (ready, address) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            ready.send(listener.local_addr().unwrap()).unwrap();
            server::serve(listener, relay, server::Config { token: TOKEN.into(), origin: Some(ORIGIN.into()) }).await.unwrap();
        });
    });
    (format!("http://{}", address.recv().unwrap()), mock)
}

fn client(base: &str, token: &str) -> RelayBackend<HttpTransport> { RelayBackend::new(AccountId::from("iCloud"), HttpTransport::new(base, token)) }

#[test]
fn an_engine_runs_unchanged_over_the_relay() {
    let (base, mock) = start();
    let mut engine = Engine::new(mail_store::Store::open_in_memory().unwrap(), Config { undo_send_secs: 0.0 });
    engine.add_backend(Arc::new(client(&base, TOKEN)));
    let account = AccountId::from("iCloud");
    let mut run = |cmd| block_on(engine.handle(cmd, 0.0));
    let opened = run(Command::OpenAccount(account.clone()));
    assert!(opened.contains(&Event::SyncState { account: account.clone(), state: mail_core::machine::account_sync::State::Idle }), "{opened:?}");
    let inbox = FolderId::from("iCloud/Inbox");
    let Some(Event::Envelopes { envelopes, .. }) = run(Command::LoadRange { folder: inbox.clone(), offset: 0, limit: 5 }).pop() else { panic!() };
    let first = envelopes[0].clone();
    assert!(matches!(run(Command::OpenMessage { id: first.id.clone(), view: ravenglass_engine::ReaderView::Text }).pop(), Some(Event::Body { .. })));
    let archived = run(Command::MoveToRole { ids: vec![first.id.clone()], role: FolderRole::Archive });
    assert!(archived.iter().any(|e| matches!(e, Event::Applied { sent: true, .. })), "{archived:?}");
    let on_server = block_on(mock.sync_folder(&FolderId::from("iCloud/Archive"), &SyncToken::default())).unwrap();
    assert!(on_server.added.iter().any(|e| e.id == first.id));
    run(Command::Send { account, raw: b"Subject: over the relay\r\n\r\nhi".to_vec(), from: Address::new("J", "j@example.com"), rcpt: vec![Address::new("O", "o@example.com")] });
    run(Command::Tick);
    assert_eq!(mock.sent.lock().unwrap().len(), 1);
}

#[test]
fn requests_without_the_token_or_on_another_version_are_refused() {
    let (base, _) = start();
    assert_eq!(block_on(client(&base, "wrong token entirely").list_folders()), Err(MailError::Auth));
    assert_eq!(block_on(client(&base, "").list_folders()), Err(MailError::Auth));
    assert!(block_on(client(&base, TOKEN).open_session()).is_ok());
    let agent = ureq::Agent::new_with_config(ureq::Agent::config_builder().http_status_as_error(false).build());
    let old = agent.get(format!("{base}/v1/accounts/iCloud/folders")).header("authorization", format!("Bearer {TOKEN}")).header("ravenglass-relay-version", "99").call().unwrap();
    assert_eq!(old.status().as_u16(), 426);
    let missing = agent.get(format!("{base}/v1/accounts/iCloud/folders")).header("authorization", format!("Bearer {TOKEN}")).call().unwrap();
    assert_eq!(missing.status().as_u16(), 426, "a request must say which protocol it speaks");
}

#[test]
fn only_the_configured_origin_gets_cors_headers() {
    let (base, _) = start();
    let agent = ureq::Agent::new_with_config(ureq::Agent::config_builder().http_status_as_error(false).build());
    let get = |origin: &str| {
        let r = agent.get(format!("{base}/v1/accounts/iCloud/folders")).header("authorization", format!("Bearer {TOKEN}")).header("ravenglass-relay-version", "1").header("origin", origin).call().unwrap();
        (r.status().as_u16(), r.headers().get("access-control-allow-origin").map(|v| v.to_str().unwrap().to_owned()))
    };
    assert_eq!(get(ORIGIN), (200, Some(ORIGIN.to_owned())));
    assert_eq!(get("https://evil.example"), (200, None), "another site's page cannot read the answer");
}

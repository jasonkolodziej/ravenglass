//! Helpers shared by the users-mode tests: a Relay on a loopback port with mock accounts and a
//! clock the test moves, and an HTTP agent that shows redirects and errors instead of following them.

#![allow(dead_code)]

use mail_auth::secrets::SecretStore;
use mail_auth::{AccountConfig, AuthKind, Incoming, Security, Server};
use mail_core::*;
use mail_mock::MockBackend;
use ravenglass_relay::crypto::{MasterKey, PasswordCost, hash_password};
use ravenglass_relay::hub::Opener;
use ravenglass_relay::server::{self, UsersConfig};
use ravenglass_relay::store::Store;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Every backend opened: its account id and the secret it was opened with.
pub type Opened = Arc<Mutex<Vec<(String, Option<String>)>>>;

pub const ORIGIN: &str = "https://mail.example.org";
pub const ALICE_PW: &str = "alice's long password";
pub const BOB_PW: &str = "bob's even longer password";

pub struct Relay {
    pub base: String,
    pub store: Arc<Store>,
    pub now: Arc<AtomicU64>,
    /// Every backend opened, with the secret it was opened with.
    pub opened: Opened,
}

impl Relay {
    pub fn advance(&self, seconds: u64) { self.now.fetch_add(seconds, Ordering::SeqCst); }
}

/// Mock accounts whose IDLE reports a new message in their Inbox every time it runs.
pub fn mock_opener(opened: Opened) -> Opener {
    Arc::new(move |config: &AccountConfig, secrets: Arc<dyn SecretStore>| {
        let secret = secrets.get(&config.id, mail_auth::SecretKind::Password).map_err(|e| e.to_string())?;
        opened.lock().unwrap().push((config.id.0.clone(), secret));
        Ok(Arc::new(Pinging(MockBackend::new(&config.id.0, 46, 20))) as Arc<dyn MailBackend>)
    })
}

/// Start a users-mode Relay with alice and bob. `tune` adjusts the configuration first.
pub fn start(tune: impl FnOnce(&mut UsersConfig, &str) + Send + 'static) -> Relay {
    start_with_store(Arc::new(Store::open_in_memory(MasterKey::generate()).unwrap()), tune)
}

pub fn start_with_store(store: Arc<Store>, tune: impl FnOnce(&mut UsersConfig, &str) + Send + 'static) -> Relay {
    store.add_user("alice", Some(&hash_password(ALICE_PW, PasswordCost::Weak))).unwrap();
    store.add_user("bob", Some(&hash_password(BOB_PW, PasswordCost::Weak))).unwrap();
    let now = Arc::new(AtomicU64::new(1_800_000_000));
    let opened = Arc::new(Mutex::new(Vec::new()));
    let mut config = UsersConfig::new(store.clone(), mock_opener(opened.clone()));
    config.origin = Some(ORIGIN.into());
    config.password_cost = PasswordCost::Weak;
    config.clock = Arc::new({ let now = now.clone(); move || now.load(Ordering::SeqCst) });
    let (ready, address) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            tune(&mut config, &base);
            ready.send(base).unwrap();
            server::serve_users(listener, config).await.unwrap();
        });
    });
    Relay { base: address.recv().unwrap(), store, now, opened }
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).timeout_global(Some(std::time::Duration::from_secs(30))).build())
}

pub struct Answer { pub status: u16, pub headers: Vec<(String, String)>, pub body: Vec<u8> }

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> { self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()) }
    pub fn all(&self, name: &str) -> Vec<&str> { self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect() }
    pub fn json(&self) -> serde_json::Value { serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null) }
    pub fn text(&self) -> String { String::from_utf8_lossy(&self.body).into_owned() }
}

/// One request. `headers` are added as given; a body makes it a request with a body.
pub fn call(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> Answer {
    let agent = agent();
    let mut response = match (method, body) {
        ("GET", _) => { let mut r = agent.get(url); for (k, v) in headers { r = r.header(*k, *v); } r.call() }
        ("DELETE", _) => { let mut r = agent.delete(url); for (k, v) in headers { r = r.header(*k, *v); } r.call() }
        ("POST", body) => { let mut r = agent.post(url); for (k, v) in headers { r = r.header(*k, *v); } r.send(body.unwrap_or_default()) }
        _ => panic!("method {method}"),
    }.unwrap();
    let headers = response.headers().iter().map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or_default().to_owned())).collect();
    let status = response.status().as_u16();
    let body = response.body_mut().with_config().limit(64 << 20).read_to_vec().unwrap_or_default();
    Answer { status, headers, body }
}

/// A signed-in browser: its cookie (`name=value`) and CSRF token.
#[derive(Clone)]
pub struct Session { pub cookie: String, pub csrf: String, pub user: serde_json::Value }

pub fn login(base: &str, username: &str, password: &str) -> Answer {
    let body = serde_json::json!({ "username": username, "password": password }).to_string();
    call("POST", &format!("{base}/v1/auth/login"), &[("content-type", "application/json")], Some(body.as_bytes()))
}

pub fn sign_in(base: &str, username: &str, password: &str) -> Session {
    let answer = login(base, username, password);
    assert_eq!(answer.status, 200, "{}", answer.text());
    let cookie = answer.header("set-cookie").unwrap().split(';').next().unwrap().to_owned();
    let json = answer.json();
    Session { cookie, csrf: json["csrf"].as_str().unwrap().to_owned(), user: json["user"].clone() }
}

impl Session {
    pub fn get(&self, base: &str, path: &str) -> Answer { call("GET", &format!("{base}{path}"), &[("cookie", &self.cookie), ("ravenglass-relay-version", "1")], None) }
    pub fn post(&self, base: &str, path: &str, body: &[u8]) -> Answer {
        call("POST", &format!("{base}{path}"), &[("cookie", &self.cookie), ("ravenglass-relay-version", "1"), ("content-type", "application/json"), ("x-ravenglass-csrf", &self.csrf)], Some(body))
    }
    pub fn delete(&self, base: &str, path: &str) -> Answer { call("DELETE", &format!("{base}{path}"), &[("cookie", &self.cookie), ("x-ravenglass-csrf", &self.csrf)], None) }
    pub fn add_account(&self, base: &str, id: &str, password: &str) -> Answer {
        let body = serde_json::json!({ "config": account(id), "secret": { "kind": "Password", "value": password } }).to_string();
        self.post(base, "/v1/me/accounts", body.as_bytes())
    }
}

pub fn account(id: &str) -> AccountConfig {
    AccountConfig {
        id: AccountId::from(id),
        name: "Tester".into(),
        address: format!("{id}@example.net"),
        username: format!("{id}@example.net"),
        incoming: Incoming::Imap(Server::new("imap.example.net", 993, Security::Tls)),
        smtp: Some(Server::new("smtp.example.net", 465, Security::Tls)),
        auth: AuthKind::Password,
        server_saves_sent: false,
    }
}

/// A mock account whose IDLE reports one new Inbox message per run.
pub struct Pinging(pub MockBackend);

impl MailBackend for Pinging {
    fn account(&self) -> &AccountId { self.0.account() }
    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> { self.0.list_folders() }
    fn sync_folder<'a>(&'a self, f: &'a FolderId, s: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> { self.0.sync_folder(f, s) }
    fn fetch_body<'a>(&'a self, m: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> { self.0.fetch_body(m) }
    fn set_flags<'a>(&'a self, m: &'a [MessageId], a: Flags, r: Flags) -> BoxFut<'a, Result<()>> { self.0.set_flags(m, a, r) }
    fn move_to<'a>(&'a self, m: &'a [MessageId], t: &'a FolderId) -> BoxFut<'a, Result<()>> { self.0.move_to(m, t) }
    fn append_draft<'a>(&'a self, r: &'a [u8]) -> BoxFut<'a, Result<MessageId>> { self.0.append_draft(r) }
    fn send<'a>(&'a self, r: &'a [u8], f: &'a Address, t: &'a [Address]) -> BoxFut<'a, Result<()>> { self.0.send(r, f, t) }
    fn idle<'a>(&'a self, folder: &'a FolderId, events: std::sync::mpsc::Sender<IdleEvent>) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { let _ = events.send(IdleEvent::Exists(folder.clone())); Ok(()) })
    }
}

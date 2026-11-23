//! Mail connections and pushes, per owner.
//!
//! An owner is a user id in users mode and [`TOKEN_OWNER`] in token mode. Every push carries
//! its owner, and a push socket forwards only its own owner's pushes, so nobody hears about
//! anyone else's mail.
//!
//! In users mode a user's accounts are opened on demand, on the first request that needs one,
//! through an [`Opener`] (normally `ravenglass_accounts::Connector` over [`UserSecrets`]), and
//! closed by [`Hub::reap`] once the user has been away for [`CLOSE_AFTER`] with no push socket.

use crate::sessions::Clock;
use crate::store::{Store, UserSecrets};
use crate::AccountSet;
use mail_auth::secrets::SecretStore;
use mail_auth::AccountConfig;
use mail_core::{AccountId, FolderRole, IdleEvent, MailBackend, block_on};
use relay_proto::{ApiError, ErrorCode, Push};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

pub type Owner = i64;
/// The one owner in token mode.
pub const TOKEN_OWNER: Owner = 0;
/// Close a user's mail connections after this long without a request or a push socket.
pub const CLOSE_AFTER: u64 = 30 * 60;
/// How long a push watcher waits before reconnecting after its connection ends.
const REWATCH: Duration = Duration::from_secs(10);

/// A push for one owner's sockets.
#[derive(Clone, Debug)]
pub struct Pushed { pub owner: Owner, pub json: String }

/// Build a backend for an account from its settings and its owner's secrets. Errors are text
/// for the client and must not contain secrets.
pub type Opener = Arc<dyn Fn(&AccountConfig, Arc<dyn SecretStore>) -> Result<Arc<dyn MailBackend>, String> + Send + Sync>;

/// The real opener: IMAP/SMTP or JMAP through `ravenglass_accounts::Connector`.
pub fn connector_opener(client_ids: ravenglass_accounts::ClientIds) -> Opener {
    Arc::new(move |config, secrets| {
        let connector = ravenglass_accounts::Connector { secrets, client_ids: client_ids.clone(), post: Some(ravenglass_accounts::http::form_post()) };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        connector.connect(config, now).map(|c| c.backend).map_err(|e| e.to_string())
    })
}

/// Hold an account's push connection until `stop` is set, tagging every push with its owner.
pub fn watch(backend: Arc<dyn MailBackend>, owner: Owner, pushes: broadcast::Sender<Pushed>, stop: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new().name("relay-push".into()).spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let inbox = block_on(backend.list_folders()).ok().and_then(|f| f.into_iter().find(|f| f.role == FolderRole::Inbox));
            if let Some(inbox) = inbox {
                let (pushed, events) = std::sync::mpsc::channel::<IdleEvent>();
                let forward = std::thread::spawn({
                    let (pushes, stop) = (pushes.clone(), stop.clone());
                    // Dropping `events` when stopped ends the IDLE at its next event or keepalive.
                    move || while !stop.load(Ordering::Relaxed) {
                        match events.recv_timeout(Duration::from_secs(1)) {
                            Ok(event) => if let Ok(json) = serde_json::to_string(&Push::from(event)) { let _ = pushes.send(Pushed { owner, json }); },
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                });
                let _ = block_on(backend.idle(&inbox.id, pushed));
                let _ = forward.join();
            }
            let mut waited = Duration::ZERO;
            while waited < REWATCH && !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
                waited += Duration::from_millis(250);
            }
        }
    });
}

struct Open { backend: Arc<dyn MailBackend>, stop: Arc<AtomicBool> }

impl Drop for Open {
    fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); }
}

#[derive(Default)]
struct UserState { open: HashMap<AccountId, Open>, last_seen: u64, sockets: usize }

pub struct Hub { store: Arc<Store>, opener: Opener, pushes: broadcast::Sender<Pushed>, clock: Clock, users: Mutex<HashMap<Owner, UserState>> }

/// What one users-mode request may reach: the ids of the user's accounts and, if the request
/// names one of them, its open backend.
pub struct UserView { ids: Vec<AccountId>, open: Option<(AccountId, Arc<dyn MailBackend>)> }

impl AccountSet for UserView {
    fn ids(&self) -> Vec<AccountId> { self.ids.clone() }
    fn backend(&self, account: &AccountId) -> Option<Arc<dyn MailBackend>> { self.open.as_ref().filter(|(id, _)| id == account).map(|(_, b)| b.clone()) }
}

fn server_error(message: &str) -> ApiError { ApiError::new(ErrorCode::Server, message) }

impl Hub {
    pub fn new(store: Arc<Store>, opener: Opener, pushes: broadcast::Sender<Pushed>, clock: Clock) -> Self {
        Self { store, opener, pushes, clock, users: Mutex::new(HashMap::new()) }
    }

    fn users(&self) -> std::sync::MutexGuard<'_, HashMap<Owner, UserState>> { self.users.lock().expect("hub lock") }

    /// The user's accounts, with `account` opened if it is one of them.
    pub async fn view(&self, user: Owner, account: Option<&AccountId>) -> Result<UserView, ApiError> {
        let now = (self.clock)();
        self.users().entry(user).or_default().last_seen = now;
        let ids: Vec<AccountId> = self.store.accounts(user).map_err(|_| server_error("the account store failed"))?.into_iter().map(|c| c.id).collect();
        let Some(account) = account.filter(|a| ids.contains(a)) else { return Ok(UserView { ids, open: None }) };
        let existing = self.users().get(&user).and_then(|u| u.open.get(account)).map(|o| o.backend.clone());
        let backend = match existing {
            Some(backend) => backend,
            None => self.open(user, account).await?,
        };
        Ok(UserView { ids, open: Some((account.clone(), backend)) })
    }

    async fn open(&self, user: Owner, account: &AccountId) -> Result<Arc<dyn MailBackend>, ApiError> {
        let config = self.store.account(user, account).map_err(|_| server_error("the account store failed"))?.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such account"))?;
        let (opener, secrets): (Opener, Arc<dyn SecretStore>) = (self.opener.clone(), Arc::new(UserSecrets::new(self.store.clone(), user)));
        let opened = tokio::task::spawn_blocking(move || opener(&config, secrets)).await.map_err(|_| server_error("opening the account failed"))?;
        let backend = opened.map_err(|e| ApiError::new(ErrorCode::Server, format!("the account could not be opened: {e}")))?;
        let mut users = self.users();
        let state = users.entry(user).or_default();
        if let Some(raced) = state.open.get(account) { return Ok(raced.backend.clone()); }
        let stop = Arc::new(AtomicBool::new(false));
        watch(backend.clone(), user, self.pushes.clone(), stop.clone());
        state.open.insert(account.clone(), Open { backend: backend.clone(), stop });
        Ok(backend)
    }

    /// Close one account's connection (it was removed).
    pub fn close(&self, user: Owner, account: &AccountId) { if let Some(state) = self.users().get_mut(&user) { state.open.remove(account); } }

    /// Close everything a user has open (they were removed or signed out everywhere).
    pub fn close_user(&self, user: Owner) { self.users().remove(&user); }

    /// Whether any of the user's accounts is open now.
    pub fn open_count(&self, user: Owner) -> usize { self.users().get(&user).map_or(0, |u| u.open.len()) }

    pub fn socket_opened(&self, user: Owner) {
        let now = (self.clock)();
        let mut users = self.users();
        let state = users.entry(user).or_default();
        state.sockets += 1;
        state.last_seen = now;
    }

    pub fn socket_closed(&self, user: Owner) {
        let now = (self.clock)();
        if let Some(state) = self.users().get_mut(&user) {
            state.sockets = state.sockets.saturating_sub(1);
            state.last_seen = now;
        }
    }

    /// Close the connections of users away for [`CLOSE_AFTER`] with no push socket.
    pub fn reap(&self) {
        let now = (self.clock)();
        self.users().retain(|_, state| state.sockets > 0 || now < state.last_seen + CLOSE_AFTER);
    }

    /// Open the user's accounts so their pushes start (a push socket connected).
    pub async fn open_all(&self, user: Owner) {
        let Ok(configs) = self.store.accounts(user) else { return };
        for config in configs {
            let _ = self.view(user, Some(&config.id)).await;
        }
    }
}

#[cfg(test)]
mod tests;

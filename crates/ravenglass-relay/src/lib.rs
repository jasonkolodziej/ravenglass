//! The Ravenglass Relay (architecture §8): the server the web build talks to, because browsers
//! cannot open IMAP or SMTP sockets. It holds the real mail connections and stores no mail.
//!
//! [`Relay::handle`] (and [`handle_with`]) map one Relay API request onto a [`MailBackend`] and
//! return the response. They are transport-free, so the client and the Relay are tested against
//! each other without sockets.
//!
//! [`server`] wraps them in HTTP and runs in one of two modes (`docs/RELAY.md`):
//!
//! - **token**: self-hosted for one person. A shared bearer token guards a fixed set of accounts.
//! - **users**: multi-user. Password (Argon2id) or OIDC sign-in, server-side sessions with a
//!   cookie and a CSRF token, per-user mail accounts whose settings and secrets are sealed with
//!   AES-256-GCM under a per-user key, and strict per-user isolation of routes and pushes.
//!
//! Both modes serve the image proxy (`/v1/img`) and the push socket (`/v1/events`).
//!
//! Not here yet: passkeys, Web Push and server search.

#[cfg(feature = "server")]
pub mod config;
#[cfg(feature = "server")]
pub mod crypto;
#[cfg(feature = "server")]
pub mod hub;
#[cfg(feature = "server")]
pub mod imgproxy;
#[cfg(feature = "server")]
pub mod jose;
#[cfg(feature = "server")]
pub mod jwks;
#[cfg(feature = "server")]
pub mod oidc;
#[cfg(feature = "server")]
pub mod outbound;
#[cfg(feature = "server")]
pub mod ratelimit;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod sessions;
#[cfg(feature = "server")]
pub mod store;

use mail_core::*;
use relay_proto::*;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct Response { pub status: u16, pub content_type: &'static str, pub body: Vec<u8> }

#[derive(Default)]
pub struct Relay { accounts: BTreeMap<AccountId, Arc<dyn MailBackend>> }

/// The accounts one request may reach: all of them in token mode, the signed-in user's in users mode.
pub trait AccountSet: Send + Sync {
    fn ids(&self) -> Vec<AccountId>;
    fn backend(&self, account: &AccountId) -> Option<Arc<dyn MailBackend>>;
}

impl AccountSet for Relay {
    fn ids(&self) -> Vec<AccountId> { self.accounts.keys().cloned().collect() }
    fn backend(&self, account: &AccountId) -> Option<Arc<dyn MailBackend>> { self.accounts.get(account).cloned() }
}

impl Relay {
    pub fn new() -> Self { Self::default() }

    pub fn backends(&self) -> impl Iterator<Item = &Arc<dyn MailBackend>> { self.accounts.values() }

    pub fn add_backend(&mut self, backend: Arc<dyn MailBackend>) { self.accounts.insert(backend.account().clone(), backend); }

    /// Serve one request. `version` is the client's [`VERSION_HEADER`] value.
    pub async fn handle(&self, version: u32, method: Method, path: &str, body: &[u8]) -> Response { handle_with(self, version, method, path, body).await }
}

/// Serve one request against `accounts`. An account outside the set is `NotFound`, exactly like
/// one that does not exist, so a user cannot learn anything about anyone else's accounts.
pub async fn handle_with(accounts: &dyn AccountSet, version: u32, method: Method, path: &str, body: &[u8]) -> Response {
    match serve(accounts, version, method, path, body).await {
        Ok(response) => response,
        Err(error) => Response { status: error.code.http_status(), content_type: "application/json", body: serde_json::to_vec(&error).unwrap_or_default() },
    }
}

async fn serve(accounts: &dyn AccountSet, version: u32, method: Method, path: &str, body: &[u8]) -> std::result::Result<Response, ApiError> {
    if !SUPPORTED_VERSIONS.contains(&version) { return Err(ApiError::new(ErrorCode::UnsupportedVersion, format!("this relay speaks {SUPPORTED_VERSIONS:?}"))); }
    let route = route(method, path).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such route"))?;
    let backend = |a: &AccountId| accounts.backend(a).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such account"));
    match route {
        Route::Session => json(&SessionOpened { protocol: version, accounts: accounts.ids() }),
        Route::Folders(a) => json(&backend(&a)?.list_folders().await?),
        Route::Sync(a, folder) => {
            let request: SyncRequest = parse(body)?;
            json(&backend(&a)?.sync_folder(&folder, &request.since).await?)
        }
        Route::Raw(a, message) => {
            let raw = backend(&a)?.fetch_body(&message).await?;
            Ok(Response { status: 200, content_type: "message/rfc822", body: raw.bytes })
        }
        Route::Ops(a) => {
            let backend = backend(&a)?;
            let request: OpsRequest = parse(body)?;
            let mut results = Vec::with_capacity(request.ops.len());
            for op in &request.ops {
                let result = match op {
                    MessageOp::SetFlags { ids, add, remove } => backend.set_flags(ids, *add, *remove).await,
                    MessageOp::Move { ids, to } => backend.move_to(ids, to).await,
                };
                let failed = result.is_err();
                results.push(result.map_err(ApiError::from));
                if failed { break; }
            }
            json(&OpsResponse { results })
        }
        Route::Send(a) => {
            let request: SendRequest = parse(body)?;
            backend(&a)?.send(&request.raw, &request.from, &request.rcpt).await?;
            json(&())
        }
        Route::Drafts(a) => json(&DraftSaved { id: backend(&a)?.append_draft(body).await? }),
    }
}

fn json<T: serde::Serialize>(value: &T) -> std::result::Result<Response, ApiError> {
    let body = serde_json::to_vec(value).map_err(|e| ApiError::new(ErrorCode::Server, e.to_string()))?;
    Ok(Response { status: 200, content_type: "application/json", body })
}

fn parse<'a, T: serde::Deserialize<'a>>(body: &'a [u8]) -> std::result::Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| ApiError::new(ErrorCode::BadRequest, e.to_string()))
}

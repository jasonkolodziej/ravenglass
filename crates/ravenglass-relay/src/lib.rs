//! The Ravenglass Relay (architecture §8): the server the web build talks to, because browsers
//! cannot open IMAP or SMTP sockets. It holds the real mail connections and stores no mail.
//!
//! This crate is the request handling only: [`Relay::handle`] maps one Relay API request onto a
//! [`MailBackend`] and returns the response. It is transport-free, so the client and the Relay
//! are tested against each other without sockets.
//!
//! [`server`] wraps it in HTTP: a shared bearer token, CORS for one origin, and a WebSocket that
//! pushes folder changes.
//!
//! Not here yet (milestone W2): passkey or OIDC sign-in with per-user sessions (one token
//! guards every account for now), encrypted token storage, the image proxy, Web Push and
//! server search.

#[cfg(feature = "server")]
pub mod server;

use mail_core::*;
use relay_proto::*;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct Response { pub status: u16, pub content_type: &'static str, pub body: Vec<u8> }

#[derive(Default)]
pub struct Relay { accounts: BTreeMap<AccountId, Arc<dyn MailBackend>> }

impl Relay {
    pub fn new() -> Self { Self::default() }

    pub fn backends(&self) -> impl Iterator<Item = &Arc<dyn MailBackend>> { self.accounts.values() }

    pub fn add_backend(&mut self, backend: Arc<dyn MailBackend>) { self.accounts.insert(backend.account().clone(), backend); }

    /// Serve one request. `version` is the client's [`VERSION_HEADER`] value.
    pub async fn handle(&self, version: u32, method: Method, path: &str, body: &[u8]) -> Response {
        match self.serve(version, method, path, body).await {
            Ok(response) => response,
            Err(error) => Response { status: error.code.http_status(), content_type: "application/json", body: serde_json::to_vec(&error).unwrap_or_default() },
        }
    }

    async fn serve(&self, version: u32, method: Method, path: &str, body: &[u8]) -> std::result::Result<Response, ApiError> {
        if !SUPPORTED_VERSIONS.contains(&version) { return Err(ApiError::new(ErrorCode::UnsupportedVersion, format!("this relay speaks {SUPPORTED_VERSIONS:?}"))); }
        let route = route(method, path).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such route"))?;
        match route {
            Route::Session => json(&SessionOpened { protocol: version, accounts: self.accounts.keys().cloned().collect() }),
            Route::Folders(a) => json(&self.backend(&a)?.list_folders().await?),
            Route::Sync(a, folder) => {
                let request: SyncRequest = parse(body)?;
                json(&self.backend(&a)?.sync_folder(&folder, &request.since).await?)
            }
            Route::Raw(a, message) => {
                let raw = self.backend(&a)?.fetch_body(&message).await?;
                Ok(Response { status: 200, content_type: "message/rfc822", body: raw.bytes })
            }
            Route::Ops(a) => {
                let backend = self.backend(&a)?;
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
                self.backend(&a)?.send(&request.raw, &request.from, &request.rcpt).await?;
                json(&())
            }
            Route::Drafts(a) => json(&DraftSaved { id: self.backend(&a)?.append_draft(body).await? }),
        }
    }

    fn backend(&self, account: &AccountId) -> std::result::Result<&Arc<dyn MailBackend>, ApiError> {
        self.accounts.get(account).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such account"))
    }
}

fn json<T: serde::Serialize>(value: &T) -> std::result::Result<Response, ApiError> {
    let body = serde_json::to_vec(value).map_err(|e| ApiError::new(ErrorCode::Server, e.to_string()))?;
    Ok(Response { status: 200, content_type: "application/json", body })
}

fn parse<'a, T: serde::Deserialize<'a>>(body: &'a [u8]) -> std::result::Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| ApiError::new(ErrorCode::BadRequest, e.to_string()))
}

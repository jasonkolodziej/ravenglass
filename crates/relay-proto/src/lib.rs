//! Relay API v1 (architecture §8, appendix C): the only contract between the web client and the
//! Ravenglass Relay. Both sides use these types, so they cannot drift; the protocol version is
//! negotiated on connect and a Relay serves the current and the previous version.
//!
//! Bodies are JSON, except a raw message, which is `message/rfc822` bytes. Ids travel in paths
//! percent-encoded ([`segment`]) because IMAP folder names contain `/`.
//!
//! One deliberate difference from the appendix sketch: message routes sit under
//! `/v1/accounts/{a}/`, so the Relay never has to guess which account an id belongs to.

use mail_core::machine::account_sync;
use mail_core::*;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
/// Versions this build can speak, newest first.
pub const SUPPORTED_VERSIONS: [u32; 1] = [PROTOCOL_VERSION];
/// Request and response header carrying the protocol version.
pub const VERSION_HEADER: &str = "ravenglass-relay-version";

/// The newest version both sides speak.
pub fn negotiate(client: &[u32]) -> Option<u32> { SUPPORTED_VERSIONS.iter().copied().find(|v| client.contains(v)) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method { Get, Post }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOpened { pub protocol: u32, pub accounts: Vec<AccountId> }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRequest { pub since: SyncToken }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageOp {
    SetFlags { ids: Vec<MessageId>, add: Flags, remove: Flags },
    Move { ids: Vec<MessageId>, to: FolderId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpsRequest { pub ops: Vec<MessageOp> }

/// One result per op, in order. Ops after a failure are not attempted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpsResponse { pub results: Vec<std::result::Result<(), ApiError>> }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendRequest { pub raw: Vec<u8>, pub from: Address, pub rcpt: Vec<Address> }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftSaved { pub id: MessageId }

/// Pushed over the session WebSocket (`/v1/events`). Carries no message content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Push {
    Exists { folder: FolderId },
    Flags { folder: FolderId },
    Expunge { folder: FolderId },
    SyncState { account: AccountId, state: account_sync::State },
}

impl From<IdleEvent> for Push {
    fn from(e: IdleEvent) -> Self {
        match e {
            IdleEvent::Exists(folder) => Push::Exists { folder },
            IdleEvent::FlagsChanged(folder) => Push::Flags { folder },
            IdleEvent::Expunged(folder) => Push::Expunge { folder },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode { Auth, Network, Server, NotFound, BadRequest, UnsupportedVersion, NotImplemented }

impl ErrorCode {
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::Auth => 401, ErrorCode::NotFound => 404, ErrorCode::BadRequest => 400, ErrorCode::UnsupportedVersion => 426,
            ErrorCode::NotImplemented => 501, ErrorCode::Network => 504, ErrorCode::Server => 502,
        }
    }
}

/// The error body of every non-2xx response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError { pub code: ErrorCode, pub message: String }

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
}

impl From<MailError> for ApiError {
    fn from(e: MailError) -> Self {
        match e {
            MailError::Auth => Self::new(ErrorCode::Auth, ""),
            MailError::Network(m) => Self::new(ErrorCode::Network, m),
            MailError::Server(m) => Self::new(ErrorCode::Server, m),
            MailError::NotFound(m) => Self::new(ErrorCode::NotFound, m),
        }
    }
}

impl From<ApiError> for MailError {
    fn from(e: ApiError) -> Self {
        match e.code {
            ErrorCode::Auth => MailError::Auth,
            ErrorCode::Network => MailError::Network(e.message),
            ErrorCode::NotFound => MailError::NotFound(e.message),
            code => MailError::Server(format!("relay {code:?}: {}", e.message)),
        }
    }
}

/// Request paths. Clients build them here and the Relay parses them with [`route`].
pub mod paths {
    use super::segment;
    use mail_core::{AccountId, FolderId, MessageId};

    pub const SESSION: &str = "/v1/session";
    pub const EVENTS: &str = "/v1/events";
    pub fn folders(a: &AccountId) -> String { format!("/v1/accounts/{}/folders", segment(&a.0)) }
    pub fn sync(a: &AccountId, f: &FolderId) -> String { format!("/v1/accounts/{}/folders/{}/sync", segment(&a.0), segment(&f.0)) }
    pub fn raw(a: &AccountId, m: &MessageId) -> String { format!("/v1/accounts/{}/messages/{}/raw", segment(&a.0), segment(&m.0)) }
    pub fn ops(a: &AccountId) -> String { format!("/v1/accounts/{}/ops", segment(&a.0)) }
    pub fn send(a: &AccountId) -> String { format!("/v1/accounts/{}/send", segment(&a.0)) }
    pub fn drafts(a: &AccountId) -> String { format!("/v1/accounts/{}/drafts", segment(&a.0)) }
}

/// A parsed request line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Session,
    Folders(AccountId),
    Sync(AccountId, FolderId),
    Raw(AccountId, MessageId),
    Ops(AccountId),
    Send(AccountId),
    Drafts(AccountId),
}

pub fn route(method: Method, path: &str) -> Option<Route> {
    let parts: Vec<String> = path.strip_prefix("/v1/")?.split('/').map(unsegment).collect::<Option<_>>()?;
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    Some(match (method, parts.as_slice()) {
        (Method::Post, ["session"]) => Route::Session,
        (Method::Get, ["accounts", a, "folders"]) => Route::Folders((*a).into()),
        (Method::Post, ["accounts", a, "folders", f, "sync"]) => Route::Sync((*a).into(), (*f).into()),
        (Method::Get, ["accounts", a, "messages", m, "raw"]) => Route::Raw((*a).into(), (*m).into()),
        (Method::Post, ["accounts", a, "ops"]) => Route::Ops((*a).into()),
        (Method::Post, ["accounts", a, "send"]) => Route::Send((*a).into()),
        (Method::Post, ["accounts", a, "drafts"]) => Route::Drafts((*a).into()),
        _ => return None,
    })
}

/// Percent-encode one path segment (RFC 3986 unreserved characters pass through).
pub fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') { out.push(b as char); } else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

/// Decode a path segment; `None` for a bad escape or invalid UTF-8.
pub fn unsegment(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b != b'%' { out.push(b); continue; }
        let hex = [bytes.next()?, bytes.next()?];
        out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests;

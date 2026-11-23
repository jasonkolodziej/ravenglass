//! Sign-in, per-user accounts and the image proxy: the contract the browser client relies on.
//!
//! # Modes
//!
//! `GET /v1/auth/config` (no credentials) answers [`AuthConfig`]: which mode the Relay runs in
//! and which sign-in methods it offers.
//!
//! - **token** (self-hosted, one person): every request carries `Authorization: Bearer <token>`.
//!   The WebSocket may carry it as `?access_token=` instead. No cookie, no CSRF token.
//! - **users** (multi-user): the browser signs in and then authenticates with the session cookie.
//!
//! # Users mode
//!
//! | Call | Request | Answer |
//! | --- | --- | --- |
//! | [`LOGIN`] | `POST`, `Content-Type: application/json`, [`LoginRequest`] | `200` [`AuthSession`] and `Set-Cookie`; `401` with a generic message on any wrong name or password; `429` with `Retry-After` while rate limited |
//! | [`OIDC_START`] | `GET` (top-level navigation) | `303` to the identity provider, plus a short-lived flow cookie |
//! | [`OIDC_CALLBACK`] | `GET` from the identity provider | `303` to the app origin and `Set-Cookie`; `400`/`403` page on failure |
//! | [`SESSION`] | `GET` | `200` [`AuthSession`] (a fresh read of the CSRF token), or `401` |
//! | [`LOGOUT`] | `POST` with [`CSRF_HEADER`] | `204`; the cookie is cleared and the session ends server-side |
//!
//! The session cookie is `HttpOnly; Secure; SameSite=Strict; Path=/`, named [`SESSION_COOKIE`]
//! (or [`DEV_SESSION_COOKIE`] without `Secure` on an explicitly configured loopback dev listener).
//! The browser never reads it; it only sends it: every `fetch` uses `credentials: "include"`.
//! Sessions end after 30 minutes idle or 12 hours in total, whichever comes first.
//!
//! Every request whose method is not `GET`, `HEAD` or `OPTIONS` (except [`LOGIN`]) must echo the
//! CSRF token from [`AuthSession::csrf`] in the [`CSRF_HEADER`] header, or it is refused with `403`.
//! A browser `Origin` header, when present, must be the configured app origin.
//!
//! CORS: only the configured app origin gets `Access-Control-Allow-Origin: <origin>` and
//! `Access-Control-Allow-Credentials: true`; allowed request headers are `authorization`,
//! `content-type`, [`crate::VERSION_HEADER`] and [`CSRF_HEADER`]; methods `GET, POST, DELETE`.
//!
//! The Relay and the app must be the same *site* (for example `relay.example.org` and
//! `mail.example.org`), because a `SameSite=Strict` cookie is not sent across sites.
//!
//! # Per-user mail accounts (users mode only)
//!
//! | Call | Request | Answer |
//! | --- | --- | --- |
//! | `GET` [`ME_ACCOUNTS`] | | `200` `Vec<mail_auth::AccountConfig>` (never secrets) |
//! | `POST` [`ME_ACCOUNTS`] | [`NewAccount`] | `200` the stored `AccountConfig`; `409` if the id is taken; `400` if the settings are unusable |
//! | `DELETE` [`me_account`] | | `204`; the account, its secrets and any open connection are gone; `404` if it is not yours |
//!
//! The mail API (`/v1/session`, `/v1/accounts/{a}/…`) and the `/v1/events` socket then work as in
//! token mode, limited to the signed-in user's accounts. These routes need the
//! [`crate::VERSION_HEADER`]; the routes in this module do not.
//!
//! # Image proxy (both modes)
//!
//! `GET /v1/img?u=<percent-encoded http(s) URL>` ([`img`]), signed in: the image bytes with their
//! `image/*` type and `Cache-Control: private`, or `400` (bad URL), `403` (address not allowed),
//! `413` (over 10 MB), `415` (not an image), `502`/`504` (upstream failure or timeout).

use mail_auth::{AccountConfig, SecretKind};
use serde::{Deserialize, Serialize};

pub const CONFIG: &str = "/v1/auth/config";
pub const LOGIN: &str = "/v1/auth/login";
pub const LOGOUT: &str = "/v1/auth/logout";
pub const SESSION: &str = "/v1/auth/session";
pub const OIDC_START: &str = "/v1/auth/oidc/start";
pub const OIDC_CALLBACK: &str = "/v1/auth/oidc/callback";
pub const ME_ACCOUNTS: &str = "/v1/me/accounts";
pub const IMG: &str = "/v1/img";

/// The header every mutating request echoes the session's CSRF token in.
pub const CSRF_HEADER: &str = "x-ravenglass-csrf";
/// The session cookie's name. The `__Host-` prefix makes browsers insist on `Secure`, `Path=/` and no `Domain`.
pub const SESSION_COOKIE: &str = "__Host-ravenglass-session";
/// The cookie name on a loopback dev listener, where `Secure` is relaxed.
pub const DEV_SESSION_COOKIE: &str = "ravenglass-session";

pub fn me_account(id: &mail_core::AccountId) -> String { format!("{ME_ACCOUNTS}/{}", crate::segment(&id.0)) }

/// The image-proxy path for a remote image URL.
pub fn img(url: &str) -> String { format!("{IMG}?u={}", crate::segment(url)) }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode { Token, Users }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthConfig {
    pub mode: AuthMode,
    /// Username and password sign-in is offered.
    pub password: bool,
    /// OIDC sign-in is offered (start it at [`OIDC_START`]).
    pub oidc: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest { pub username: String, pub password: String }

impl std::fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("LoginRequest").finish_non_exhaustive() }
}

/// Who is signed in, as the Relay knows them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUser {
    /// Stable, opaque user id.
    pub id: String,
    /// The username, or the email address from the identity provider.
    pub name: String,
}

/// The answer to a sign-in and to [`SESSION`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSession { pub csrf: String, pub user: SessionUser }

impl std::fmt::Debug for AuthSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("AuthSession").field("user", &self.user).finish_non_exhaustive() }
}

/// What a secret is, on the wire. Mirrors [`SecretKind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretKindWire { Password, RefreshToken }

impl From<SecretKindWire> for SecretKind {
    fn from(k: SecretKindWire) -> Self { match k { SecretKindWire::Password => SecretKind::Password, SecretKindWire::RefreshToken => SecretKind::RefreshToken } }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSecret { pub kind: SecretKindWire, pub value: String }

impl std::fmt::Debug for AccountSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("AccountSecret").field("kind", &self.kind).finish_non_exhaustive() }
}

/// `POST /v1/me/accounts`: an account's settings and the secret it signs in with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewAccount { pub config: AccountConfig, pub secret: AccountSecret }

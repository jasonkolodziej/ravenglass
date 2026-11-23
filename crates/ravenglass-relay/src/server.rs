//! The Relay over HTTP (architecture §8), in token mode ([`serve`]) or users mode ([`serve_users`]).
//! The contract the browser relies on is documented in `relay_proto::auth`.
//!
//! Every response carries the security headers ([`secure_headers`]) and, for the configured app
//! origin only, CORS headers that allow credentials. The WebSocket at `/v1/events` pushes folder
//! changes for the caller's own accounts only and carries no message content.

mod auth;
mod me;

use crate::hub::{Opener, Owner, Pushed, TOKEN_OWNER};
use crate::imgproxy::{self, ImgError};
use crate::outbound::{AddressPolicy, FetchError};
use crate::sessions::{Clock, system_clock};
use crate::store::Store;
use crate::{Relay, crypto};
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method as HttpMethod, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use relay_proto::auth as contract;
use relay_proto::{ApiError, ErrorCode, Method, Route, SUPPORTED_VERSIONS, VERSION_HEADER, paths};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Largest request body: a 50 MB message plus JSON overhead.
const MAX_BODY: usize = 96 << 20;

/// Token mode: one person, one shared secret.
pub struct Config {
    /// The secret every request presents as `Authorization: Bearer …`. At least 16 characters.
    pub token: String,
    /// The web app's origin, the only one allowed to call from a browser. `None`: no browser access.
    pub origin: Option<String>,
}

/// OIDC sign-in settings.
pub struct OidcConfig {
    /// The issuer URL exactly as the provider's discovery document states it.
    pub issuer: String,
    pub client: crate::oidc::Client,
    /// Check ID-token signatures against the provider's published keys. Off: trust the token
    /// because it came straight from the token endpoint over TLS (OIDC Core 3.1.3.7).
    pub verify_signatures: bool,
}

/// Users mode: many people, each with their own accounts.
pub struct UsersConfig {
    /// The web app's origin: the only one allowed to call from a browser, and where OIDC sign-in returns to.
    pub origin: Option<String>,
    pub store: Arc<Store>,
    /// Builds a backend for an account; [`crate::hub::connector_opener`] for real mail.
    pub opener: Opener,
    /// Offer username and password sign-in.
    pub password_login: bool,
    pub oidc: Option<OidcConfig>,
    /// Drop `Secure` from cookies. Only for a listener bound to loopback; [`serve_users`] refuses it otherwise.
    pub insecure_dev_cookies: bool,
    /// Take the client address for rate limiting from the last `X-Forwarded-For` entry (behind a reverse proxy).
    pub trust_forwarded_for: bool,
    pub clock: Clock,
    /// Which addresses the image proxy may fetch from. [`imgproxy::public_only`] unless testing.
    pub image_policy: AddressPolicy,
    /// Argon2 cost for the dummy hash compared against when a username does not exist.
    pub password_cost: crypto::PasswordCost,
}

impl UsersConfig {
    pub fn new(store: Arc<Store>, opener: Opener) -> Self {
        Self {
            origin: None, store, opener, password_login: true, oidc: None, insecure_dev_cookies: false, trust_forwarded_for: false,
            clock: system_clock(), image_policy: imgproxy::public_only(), password_cost: crypto::PasswordCost::Standard,
        }
    }
}

enum Mode { Token { token: String, relay: Arc<Relay> }, Users(Box<auth::Users>) }

struct Inner { mode: Mode, origin: Option<String>, pushes: broadcast::Sender<Pushed>, image_policy: AddressPolicy, clock: Clock, trust_forwarded_for: bool }

#[derive(Clone)]
struct Shared(Arc<Inner>);

impl std::ops::Deref for Shared {
    type Target = Inner;
    fn deref(&self) -> &Inner { &self.0 }
}

/// Who is calling.
#[derive(Clone)]
enum Principal {
    /// Token mode, with the right token.
    Owner,
    /// Users mode: the session's user, its CSRF token and the cookie token that found it.
    User { id: i64, csrf: String, cookie: String },
}

impl Principal {
    fn owner(&self) -> Owner { match self { Principal::Owner => TOKEN_OWNER, Principal::User { id, .. } => *id } }
}

/// The token-mode router. Starts one push watcher thread per account.
pub fn router(relay: Relay, config: Config) -> Result<Router, &'static str> {
    if config.token.len() < 16 { return Err("the relay token must be at least 16 characters"); }
    let (pushes, _) = broadcast::channel(256);
    for backend in relay.backends() { crate::hub::watch(backend.clone(), TOKEN_OWNER, pushes.clone(), Arc::default()); }
    let mode = Mode::Token { token: config.token, relay: Arc::new(relay) };
    Ok(build(Inner { mode, origin: config.origin, pushes, image_policy: imgproxy::public_only(), clock: system_clock(), trust_forwarded_for: false }))
}

/// The users-mode router.
pub fn users_router(config: UsersConfig) -> Router { build_users(config).0 }

fn build_users(config: UsersConfig) -> (Router, Shared) {
    let (pushes, _) = broadcast::channel(1024);
    let users = auth::Users::new(&config, pushes.clone());
    let inner = Inner { mode: Mode::Users(Box::new(users)), origin: config.origin, pushes, image_policy: config.image_policy, clock: config.clock, trust_forwarded_for: config.trust_forwarded_for };
    let shared = Shared(Arc::new(inner));
    (routes(shared.clone()), shared)
}

fn build(inner: Inner) -> Router { routes(Shared(Arc::new(inner))) }

fn routes(shared: Shared) -> Router {
    Router::new()
        .route(paths::EVENTS, get(events))
        .route(contract::CONFIG, get(auth::config))
        .route(contract::LOGIN, post(auth::login))
        .route(contract::LOGOUT, post(auth::logout))
        .route(contract::SESSION, get(auth::session))
        .route(contract::OIDC_START, get(auth::oidc_start))
        .route(contract::OIDC_CALLBACK, get(auth::oidc_callback))
        .route(contract::ME_ACCOUNTS, get(me::list).post(me::add))
        .route(&format!("{}/{{id}}", contract::ME_ACCOUNTS), delete(me::remove))
        .route(contract::IMG, get(image))
        .fallback(api)
        .layer(axum::middleware::from_fn_with_state(shared.clone(), outer))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(shared)
}

/// Serve token mode until the listener fails.
pub async fn serve(listener: tokio::net::TcpListener, relay: Relay, config: Config) -> std::io::Result<()> {
    let router = router(relay, config).map_err(std::io::Error::other)?;
    axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await
}

/// Serve users mode until the listener fails. Closes idle users' mail connections every minute.
pub async fn serve_users(listener: tokio::net::TcpListener, config: UsersConfig) -> std::io::Result<()> {
    if config.insecure_dev_cookies && !listener.local_addr()?.ip().is_loopback() {
        return Err(std::io::Error::other("insecure dev cookies are only allowed on a loopback listener"));
    }
    if !config.password_login && config.oidc.is_none() { return Err(std::io::Error::other("users mode needs password sign-in or OIDC")); }
    if config.oidc.as_ref().is_some_and(|o| !o.verify_signatures) {
        eprintln!("warning: OIDC ID-token signatures are not checked (RAVENGLASS_RELAY_OIDC_VERIFY_SIGNATURES=false); trusting the TLS connection to the token endpoint");
    }
    let (router, shared) = build_users(config);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Mode::Users(users) = &shared.mode { users.hub.reap(); }
        }
    });
    axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await
}

// Responses.

fn error(code: ErrorCode, message: &str) -> Response {
    let body = serde_json::to_vec(&ApiError::new(code, message)).unwrap_or_default();
    (StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn api_error(e: ApiError) -> Response { error(e.code, &e.message) }

/// A refused request, small enough to return in a `Result`; it becomes an [`error`] response.
struct Refusal(ErrorCode, &'static str);

impl IntoResponse for Refusal {
    fn into_response(self) -> Response { error(self.0, self.1) }
}

fn json<T: serde::Serialize>(value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Err(_) => error(ErrorCode::Server, "could not encode the answer"),
    }
}

/// Headers on every response: nothing here is a document to render, sniff or embed.
pub fn secure_headers(headers: &mut HeaderMap) {
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'; sandbox"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
}

/// Answer preflights; add CORS for the configured origin and the security headers to everything.
async fn outer(State(shared): State<Shared>, request: Request, next: Next) -> Response {
    let origin = request.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let mut response = if request.method() == HttpMethod::OPTIONS { StatusCode::NO_CONTENT.into_response() } else { next.run(request).await };
    if let (Some(allowed), Some(origin)) = (&shared.origin, origin)
        && *allowed == origin
        && let Ok(value) = HeaderValue::from_str(&origin)
    {
        let h = response.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        h.insert(header::ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
        h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("authorization, content-type, ravenglass-relay-version, x-ravenglass-csrf"));
        h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, DELETE"));
        h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    }
    response.headers_mut().append(header::VARY, HeaderValue::from_static("origin"));
    secure_headers(response.headers_mut());
    response
}

// Authentication.

impl Shared {
    fn now(&self) -> u64 { (self.clock)() }

    /// Who the request is from, or `None`. Users mode accepts only the session cookie; token
    /// mode only the bearer token.
    fn principal(&self, headers: &HeaderMap) -> Option<Principal> {
        match &self.mode {
            Mode::Token { token, .. } => {
                let presented = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
                presented.filter(|t| crypto::same(t.as_bytes(), token.as_bytes())).map(|_| Principal::Owner)
            }
            Mode::Users(users) => {
                let cookie = cookie(headers, users.cookie_name())?;
                let session = users.sessions.touch(&cookie, self.now())?;
                // A user removed with `ravenglass-relay user remove` loses access at once.
                if !matches!(users.store.user(session.user), Ok(Some(_))) {
                    users.sessions.end_user(session.user);
                    users.hub.close_user(session.user);
                    return None;
                }
                Some(Principal::User { id: session.user, csrf: session.csrf, cookie })
            }
        }
    }

    /// The browser's `Origin`, if it sent one, must be the app's.
    fn origin_ok(&self, headers: &HeaderMap) -> bool {
        match headers.get(header::ORIGIN).map(|v| v.to_str()) {
            None => true,
            Some(Ok(origin)) => self.origin.as_deref() == Some(origin),
            Some(Err(_)) => false,
        }
    }

    /// Authenticate, and for a mutating request in users mode, check its CSRF token and origin.
    fn authorize(&self, method: &HttpMethod, headers: &HeaderMap) -> Result<Principal, Refusal> {
        let principal = self.principal(headers).ok_or(Refusal(ErrorCode::Auth, "sign in first"))?;
        if let Principal::User { csrf, .. } = &principal {
            if !self.origin_ok(headers) { return Err(Refusal(ErrorCode::Forbidden, "this origin may not call the relay")); }
            let safe = matches!(*method, HttpMethod::GET | HttpMethod::HEAD | HttpMethod::OPTIONS);
            let presented = headers.get(contract::CSRF_HEADER).map(|v| v.as_bytes()).unwrap_or_default();
            if !safe && !crypto::same(presented, csrf.as_bytes()) { return Err(Refusal(ErrorCode::Forbidden, "missing or wrong CSRF token")); }
        }
        Ok(principal)
    }

    fn users(&self) -> Option<&auth::Users> { match &self.mode { Mode::Users(u) => Some(u), Mode::Token { .. } => None } }
}

/// The value of one cookie.
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|pair| {
        let (k, v) = pair.trim().split_once('=')?;
        (k == name && !v.is_empty()).then(|| v.to_owned())
    })
}

// The mail API.

fn account_of(route: &Route) -> Option<&mail_core::AccountId> {
    match route {
        Route::Session => None,
        Route::Folders(a) | Route::Sync(a, _) | Route::Raw(a, _) | Route::Ops(a) | Route::Send(a) | Route::Drafts(a) => Some(a),
    }
}

async fn api(State(shared): State<Shared>, method: HttpMethod, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let principal = match shared.authorize(&method, &headers) { Ok(p) => p, Err(refused) => return refused.into_response() };
    let version = headers.get(VERSION_HEADER).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let method = match method { HttpMethod::GET => Method::Get, HttpMethod::POST => Method::Post, _ => return error(ErrorCode::NotFound, "no such route") };
    let answer = match (&shared.mode, principal) {
        (Mode::Token { relay, .. }, _) => relay.handle(version, method, uri.path(), &body).await,
        (Mode::Users(users), Principal::User { id, .. }) => {
            let route = relay_proto::route(method, uri.path());
            let account = route.as_ref().filter(|_| SUPPORTED_VERSIONS.contains(&version)).and_then(account_of);
            match users.hub.view(id, account).await {
                Ok(view) => crate::handle_with(&view, version, method, uri.path(), &body).await,
                Err(e) => return api_error(e),
            }
        }
        (Mode::Users(_), Principal::Owner) => return error(ErrorCode::Auth, "sign in first"),
    };
    (StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), [(header::CONTENT_TYPE, answer.content_type)], answer.body).into_response()
}

// Pushes.

/// Browsers cannot set headers on a WebSocket: in token mode the token may come as
/// `?access_token=`; in users mode the cookie comes along and the origin is checked.
async fn events(State(shared): State<Shared>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>, upgrade: WebSocketUpgrade) -> Response {
    let principal = match &shared.mode {
        Mode::Token { token, .. } if query.get("access_token").is_some_and(|t| crypto::same(t.as_bytes(), token.as_bytes())) => Some(Principal::Owner),
        _ => shared.principal(&headers),
    };
    let Some(principal) = principal else { return error(ErrorCode::Auth, "sign in first") };
    if matches!(principal, Principal::User { .. }) && !shared.origin_ok(&headers) { return error(ErrorCode::Forbidden, "this origin may not call the relay"); }
    let pushes = shared.pushes.subscribe();
    if let (Some(users), Principal::User { id, .. }) = (shared.users(), &principal) {
        users.hub.socket_opened(*id);
        let shared = shared.clone();
        let id = *id;
        tokio::spawn(async move { if let Some(users) = shared.users() { users.hub.open_all(id).await; } });
    }
    upgrade.on_upgrade(move |socket| push_loop(shared, principal, socket, pushes))
}

async fn push_loop(shared: Shared, principal: Principal, mut socket: WebSocket, mut pushes: broadcast::Receiver<Pushed>) {
    let owner = principal.owner();
    let mut check = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        tokio::select! {
            pushed = pushes.recv() => match pushed {
                Ok(push) if push.owner == owner => { if socket.send(Message::Text(push.json.into())).await.is_err() { break; } }
                Ok(_) => {}
                // A slow client missed some pushes; it re-syncs on the next one anyway.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => if !matches!(incoming, Some(Ok(_))) { break },
            // A signed-out or expired session stops receiving at once.
            _ = check.tick() => if let (Some(users), Principal::User { id, cookie, .. }) = (shared.users(), &principal)
                && (!users.sessions.alive(cookie, shared.now()) || !matches!(users.store.user(*id), Ok(Some(_)))) { break },
        }
    }
    if let (Some(users), Principal::User { id, .. }) = (shared.users(), &principal) { users.hub.socket_closed(*id); }
}

// The image proxy.

async fn image(State(shared): State<Shared>, method: HttpMethod, headers: HeaderMap, Query(query): Query<HashMap<String, String>>) -> Response {
    if let Err(refused) = shared.authorize(&method, &headers) { return refused.into_response(); }
    let Some(url) = query.get("u") else { return error(ErrorCode::BadRequest, "give the image URL as ?u=") };
    match imgproxy::fetch(url, &shared.image_policy).await {
        Ok(image) => {
            let content_type = HeaderValue::from_str(&image.content_type).unwrap_or(HeaderValue::from_static("application/octet-stream"));
            ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400"))], image.bytes).into_response()
        }
        Err(e) => {
            let (code, message) = match e {
                ImgError::Fetch(FetchError::BadUrl) => (ErrorCode::BadRequest, "only http and https image URLs can be fetched"),
                ImgError::Fetch(FetchError::Forbidden) => (ErrorCode::Forbidden, "that address is not on the public internet"),
                ImgError::Fetch(FetchError::TooLarge) => (ErrorCode::TooLarge, "the image is larger than 10 MB"),
                ImgError::NotAnImage => (ErrorCode::UnsupportedType, "that is not an image"),
                ImgError::Fetch(FetchError::Timeout) => (ErrorCode::Network, "the image server took too long"),
                ImgError::TooManyRedirects => (ErrorCode::Server, "too many redirects"),
                ImgError::Status(_) => (ErrorCode::Server, "the image server refused"),
                ImgError::Fetch(_) => (ErrorCode::Server, "the image server could not be reached"),
            };
            error(code, message)
        }
    }
}

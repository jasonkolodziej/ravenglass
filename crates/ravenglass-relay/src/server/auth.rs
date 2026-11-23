//! Sign-in, sign-out and the session, for users mode (contract in `relay_proto::auth`).

use super::{Mode, Principal, Refusal, Shared, UsersConfig, error, json};
use crate::crypto::{self, PasswordCost};
use crate::hub::{Hub, Pushed};
use crate::jose;
use crate::jwks::{self, KeyCache, Plan};
use crate::oidc::{self, Effect, Event, OidcError, Provider};
use crate::outbound::{self, AddressPolicy, Request as Outgoing};
use crate::ratelimit::Limiter;
use crate::sessions::{ABSOLUTE, Sessions};
use crate::store::Store;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use relay_proto::auth::{self as contract, AuthConfig, AuthMode, AuthSession, LoginRequest, SessionUser};
use relay_proto::ErrorCode;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Failed sign-ins a username may have before backoff starts.
const FREE_PER_NAME: u32 = 5;
/// Failed sign-ins an address may have before backoff starts (it may be shared, e.g. a NAT).
const FREE_PER_ADDRESS: u32 = 20;
const MAX_LOGIN_BODY: usize = 16 << 10;
const MAX_NAME: usize = 256;
const MAX_PASSWORD: usize = 1024;
/// OIDC flows waiting for their callback, at most.
const MAX_FLOWS: usize = 10_000;
const OIDC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const WRONG_LOGIN: &str = "wrong username or password";

pub struct Oidc {
    issuer: String,
    client: oidc::Client,
    provider: tokio::sync::Mutex<Option<Provider>>,
    verify_signatures: bool,
    /// Held across a key fetch, so concurrent sign-ins share one fetch.
    keys: tokio::sync::Mutex<KeyCache>,
    flows: Mutex<HashMap<String, (oidc::State, u64)>>,
}

pub struct Users {
    pub store: Arc<Store>,
    pub hub: Hub,
    pub sessions: Sessions,
    by_name: Limiter,
    by_address: Limiter,
    password_login: bool,
    oidc: Option<Oidc>,
    secure: bool,
    /// Compared against when the username does not exist, so both cases take the same time.
    dummy_hash: String,
}

impl Users {
    pub fn new(config: &UsersConfig, pushes: broadcast::Sender<Pushed>) -> Self {
        let oidc = config.oidc.as_ref().map(|o| Oidc {
            issuer: o.issuer.clone(),
            client: o.client.clone(),
            provider: tokio::sync::Mutex::new(None),
            verify_signatures: o.verify_signatures,
            keys: tokio::sync::Mutex::new(KeyCache::default()),
            flows: Mutex::new(HashMap::new()),
        });
        Self {
            store: config.store.clone(),
            hub: Hub::new(config.store.clone(), config.opener.clone(), pushes, config.clock.clone()),
            sessions: Sessions::default(),
            by_name: Limiter::new(FREE_PER_NAME),
            by_address: Limiter::new(FREE_PER_ADDRESS),
            password_login: config.password_login,
            oidc,
            secure: !config.insecure_dev_cookies,
            dummy_hash: crypto::hash_password("ravenglass dummy password", if config.password_cost == PasswordCost::Weak { PasswordCost::Weak } else { PasswordCost::Standard }),
        }
    }

    pub fn cookie_name(&self) -> &'static str { if self.secure { contract::SESSION_COOKIE } else { contract::DEV_SESSION_COOKIE } }

    fn flow_cookie_name(&self) -> &'static str { if self.secure { "__Secure-ravenglass-oidc" } else { "ravenglass-oidc" } }

    fn secure_attr(&self) -> &'static str { if self.secure { "; Secure" } else { "" } }

    fn session_cookie(&self, token: &str) -> String { format!("{}={token}; HttpOnly{}; SameSite=Strict; Path=/; Max-Age={ABSOLUTE}", self.cookie_name(), self.secure_attr()) }

    fn clear_session_cookie(&self) -> String { format!("{}=; HttpOnly{}; SameSite=Strict; Path=/; Max-Age=0", self.cookie_name(), self.secure_attr()) }

    /// The OIDC flow cookie is `Lax`: the callback is a cross-site navigation from the identity provider.
    fn flow_cookie(&self, binding: &str, max_age: u64) -> String {
        format!("{}={binding}; HttpOnly{}; SameSite=Lax; Path=/v1/auth/oidc; Max-Age={max_age}", self.flow_cookie_name(), self.secure_attr())
    }

    fn signed_in(&self, user: &crate::store::User, now: u64) -> Response {
        let (token, session) = self.sessions.create(user.id, now);
        let mut response = json(&AuthSession { csrf: session.csrf, user: SessionUser { id: user.id.to_string(), name: user.name.clone() } });
        if let Ok(cookie) = HeaderValue::from_str(&self.session_cookie(&token)) { response.headers_mut().append(header::SET_COOKIE, cookie); }
        response
    }
}

fn users_only(shared: &Shared) -> Result<&Users, Refusal> {
    match &shared.mode { Mode::Users(users) => Ok(users), Mode::Token { .. } => Err(Refusal(ErrorCode::NotFound, "this relay uses a shared token")) }
}

pub async fn config(State(shared): State<Shared>) -> Response {
    json(&match &shared.mode {
        Mode::Token { .. } => AuthConfig { mode: AuthMode::Token, password: false, oidc: false },
        Mode::Users(users) => AuthConfig { mode: AuthMode::Users, password: users.password_login, oidc: users.oidc.is_some() },
    })
}

fn client_address(shared: &Shared, request: &Request) -> String {
    let forwarded = shared.trust_forwarded_for.then(|| request.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit(',').next()).map(|v| v.trim().to_owned())).flatten();
    forwarded.filter(|v| !v.is_empty())
        .or_else(|| request.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip().to_string()))
        .unwrap_or_else(|| "unknown".into())
}

fn rate_limited(seconds: u64) -> Response {
    let mut response = error(ErrorCode::RateLimited, "too many attempts; wait and try again");
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) { response.headers_mut().insert(header::RETRY_AFTER, value); }
    response
}

pub async fn login(State(shared): State<Shared>, request: Request) -> Response {
    let users = match users_only(&shared) { Ok(u) => u, Err(r) => return r.into_response() };
    if !users.password_login { return error(ErrorCode::NotFound, "password sign-in is off"); }
    if !shared.origin_ok(request.headers()) { return error(ErrorCode::Forbidden, "this origin may not call the relay"); }
    // JSON only: a cross-site HTML form cannot send it without a preflight.
    let is_json = request.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json")));
    if !is_json { return error(ErrorCode::UnsupportedType, "send JSON"); }
    let address = format!("ip:{}", client_address(&shared, &request));
    let Ok(body) = axum::body::to_bytes(request.into_body(), MAX_LOGIN_BODY).await else { return error(ErrorCode::TooLarge, "the request is too large") };
    let Ok(login) = serde_json::from_slice::<LoginRequest>(&body) else { return error(ErrorCode::BadRequest, "send {\"username\", \"password\"}") };
    if login.username.is_empty() || login.username.len() > MAX_NAME || login.password.len() > MAX_PASSWORD { return error(ErrorCode::Auth, WRONG_LOGIN); }
    let name_key = format!("name:{}", login.username.to_lowercase());
    let now = shared.now();
    for key in [&address, &name_key] {
        let limiter = if key.starts_with("ip:") { &users.by_address } else { &users.by_name };
        if let Err(wait) = limiter.check(key, now) { return rate_limited(wait); }
    }
    let (store, dummy) = (users.store.clone(), users.dummy_hash.clone());
    let checked = tokio::task::spawn_blocking(move || {
        let found = store.password_of(&login.username).ok().flatten();
        let hash = found.as_ref().map_or(dummy.as_str(), |(_, hash)| hash.as_str());
        let matches = crypto::verify_password(&login.password, hash);
        found.filter(|_| matches).map(|(user, _)| user)
    }).await.ok().flatten();
    match checked {
        Some(user) => {
            users.by_name.succeed(&name_key);
            users.signed_in(&user, now)
        }
        None => {
            users.by_name.fail(&name_key, now);
            users.by_address.fail(&address, now);
            error(ErrorCode::Auth, WRONG_LOGIN)
        }
    }
}

pub async fn logout(State(shared): State<Shared>, method: axum::http::Method, headers: HeaderMap) -> Response {
    let users = match users_only(&shared) { Ok(u) => u, Err(r) => return r.into_response() };
    let principal = match shared.authorize(&method, &headers) { Ok(p) => p, Err(refused) => return refused.into_response() };
    if let Principal::User { cookie, .. } = principal { users.sessions.end(&cookie); }
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(cookie) = HeaderValue::from_str(&users.clear_session_cookie()) { response.headers_mut().append(header::SET_COOKIE, cookie); }
    response
}

pub async fn session(State(shared): State<Shared>, method: axum::http::Method, headers: HeaderMap) -> Response {
    let users = match users_only(&shared) { Ok(u) => u, Err(r) => return r.into_response() };
    let Ok(Principal::User { id, csrf, .. }) = shared.authorize(&method, &headers) else { return error(ErrorCode::Auth, "sign in first") };
    match users.store.user(id) {
        Ok(Some(user)) => json(&AuthSession { csrf, user: SessionUser { id: user.id.to_string(), name: user.name } }),
        _ => error(ErrorCode::Auth, "sign in first"),
    }
}

// OIDC.

/// Outbound calls to the identity provider: any address (the issuer is the operator's choice),
/// https unless the endpoint is on this machine, 1 MB at most, 10 s at most.
async fn idp(request: Outgoing) -> Result<outbound::Reply, OidcError> {
    if !oidc::secure_endpoint(request.url.as_str()) { return Err(OidcError::Discovery("every endpoint must be https")); }
    let any: AddressPolicy = Arc::new(|_| true);
    let call = async {
        let address = outbound::resolve(&request.url, &any).await.ok()?;
        outbound::send(&request, address, 1 << 20).await.ok()
    };
    tokio::time::timeout(OIDC_TIMEOUT, call).await.ok().flatten().ok_or(OidcError::Discovery("the identity provider cannot be reached"))
}

async fn provider(oidc: &Oidc) -> Result<Provider, OidcError> {
    let mut cached = oidc.provider.lock().await;
    if let Some(provider) = cached.as_ref() { return Ok(provider.clone()); }
    let url = url::Url::parse(&oidc::discovery_url(&oidc.issuer)).map_err(|_| OidcError::Discovery("the issuer is not a URL"))?;
    let reply = idp(Outgoing::get(url)).await?;
    if reply.status != 200 { return Err(OidcError::Discovery("no discovery document")); }
    let provider = oidc::parse_discovery(&String::from_utf8_lossy(&reply.body), &oidc.issuer)?;
    *cached = Some(provider.clone());
    Ok(provider)
}

/// Fetch the provider's key set into the cache. `None` when it cannot be had; the failure is
/// remembered, so the next attempt waits [`jwks::MIN_REFETCH`].
async fn fetch_keys(cache: &mut KeyCache, provider: &Provider, now: u64) -> Option<Arc<jose::KeySet>> {
    let fetched = async {
        let url = url::Url::parse(provider.jwks_uri.as_deref()?).ok()?;
        let mut request = Outgoing::get(url);
        request.headers.push(("accept", "application/jwk-set+json, application/json".into()));
        let reply = idp(request).await.ok()?;
        if reply.status != 200 { return None; }
        Some((jose::parse_key_set(&reply.body)?, jwks::lifetime(reply.header("cache-control"))))
    };
    match fetched.await {
        Some((keys, lifetime)) => { cache.fetched(keys, lifetime, now); cache.keys() }
        None => { cache.failed(now); None }
    }
}

/// Check an ID token's signature against the provider's keys, refetching them once if the
/// token names a key the cached set does not have. Never logs the token.
async fn signature_valid(oidc: &Oidc, provider: &Provider, id_token: &str, now: u64) -> bool {
    if !oidc.verify_signatures { return true; }
    let mut cache = oidc.keys.lock().await;
    let keys = match cache.plan(now) {
        Plan::Use => cache.keys(),
        Plan::Fetch => match fetch_keys(&mut cache, provider, now).await { Some(keys) => Some(keys), None => cache.keys() },
        Plan::Refuse => None,
    };
    let Some(keys) = keys else { return false };
    match jose::verify(id_token, &keys) {
        Ok(_) => true,
        Err(jose::JoseError::UnknownKey) if cache.plan_unknown_key(now) == Plan::Fetch => match fetch_keys(&mut cache, provider, now).await {
            Some(keys) => jose::verify(id_token, &keys).is_ok(),
            None => false,
        },
        Err(_) => false,
    }
}

fn page(status: StatusCode, text: &str) -> Response { (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text.to_owned()).into_response() }

fn see_other(location: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let Ok(value) = HeaderValue::from_str(location) { response.headers_mut().insert(header::LOCATION, value); }
    response
}

pub async fn oidc_start(State(shared): State<Shared>) -> Response {
    let users = match users_only(&shared) { Ok(u) => u, Err(r) => return r.into_response() };
    let Some(oidc) = &users.oidc else { return error(ErrorCode::NotFound, "OIDC sign-in is off") };
    let provider = match provider(oidc).await { Ok(p) => p, Err(e) => return page(StatusCode::BAD_GATEWAY, &e.to_string()) };
    let now = shared.now();
    let (flow, url) = oidc::start(&provider, &oidc.client, now);
    let oidc::State::AwaitingCallback { state, binding, .. } = &flow else { return page(StatusCode::INTERNAL_SERVER_ERROR, "sign-in could not start") };
    let cookie = users.flow_cookie(binding, oidc::FLOW_LIFETIME);
    {
        let mut flows = oidc.flows.lock().expect("flows lock");
        flows.retain(|_, (_, started)| now < *started + oidc::FLOW_LIFETIME);
        if flows.len() >= MAX_FLOWS { return page(StatusCode::SERVICE_UNAVAILABLE, "too many sign-ins in progress; try again shortly"); }
        flows.insert(state.clone(), (flow.clone(), now));
    }
    let mut response = see_other(&url);
    if let Ok(value) = HeaderValue::from_str(&cookie) { response.headers_mut().append(header::SET_COOKIE, value); }
    response
}

pub async fn oidc_callback(State(shared): State<Shared>, uri: axum::http::Uri, headers: HeaderMap) -> Response {
    let users = match users_only(&shared) { Ok(u) => u, Err(r) => return r.into_response() };
    let Some(oidc) = &users.oidc else { return error(ErrorCode::NotFound, "OIDC sign-in is off") };
    let query = uri.query().unwrap_or_default().to_owned();
    let state = url::form_urlencoded::parse(query.as_bytes()).find(|(k, _)| k == "state").map(|(_, v)| v.into_owned()).unwrap_or_default();
    // Single use: the flow is gone whatever happens next.
    let flow = oidc.flows.lock().expect("flows lock").remove(&state).map(|(flow, _)| flow);
    let outcome = match flow {
        None => Err(OidcError::WrongState),
        Some(flow) => finish(&shared, users, oidc, flow, query, &headers).await,
    };
    let clear = users.flow_cookie("", 0);
    let mut response = match outcome {
        Ok(signed_in) => signed_in,
        Err(e) => page(if e == OidcError::NotAllowed { StatusCode::FORBIDDEN } else { StatusCode::BAD_REQUEST }, &e.to_string()),
    };
    if let Ok(value) = HeaderValue::from_str(&clear) { response.headers_mut().append(header::SET_COOKIE, value); }
    response
}

/// Drive the flow to its end: callback, token request, userinfo, then a session.
async fn finish(shared: &Shared, users: &Users, oidc: &Oidc, flow: oidc::State, query: String, headers: &HeaderMap) -> Result<Response, OidcError> {
    let provider = provider(oidc).await?;
    let binding = super::cookie(headers, users.flow_cookie_name()).unwrap_or_default();
    let mut state = flow;
    let mut event = Event::Callback { query, binding, now: shared.now() };
    loop {
        let (next, effect) = state.step(&provider, &oidc.client, event);
        state = next;
        event = match effect {
            Some(Effect::PostToken { url, body, basic }) => {
                let mut request = Outgoing::get(url::Url::parse(&url).map_err(|_| OidcError::Token("bad token endpoint"))?);
                request.method = hyper::Method::POST;
                request.headers.push(("content-type", "application/x-www-form-urlencoded".into()));
                request.headers.push(("accept", "application/json".into()));
                if let Some(basic) = basic { request.headers.push(("authorization", format!("Basic {basic}"))); }
                request.body = body.into_bytes();
                let reply = idp(request).await?;
                Event::Tokens { status: reply.status, body: String::from_utf8_lossy(&reply.body).into_owned(), now: shared.now() }
            }
            Some(Effect::CheckSignature { id_token }) => Event::Signature { valid: signature_valid(oidc, &provider, &id_token, shared.now()).await },
            Some(Effect::GetUserInfo { url, access_token }) => {
                let mut request = Outgoing::get(url::Url::parse(&url).map_err(|_| OidcError::UserInfo("bad userinfo endpoint"))?);
                request.headers.push(("authorization", format!("Bearer {access_token}")));
                request.headers.push(("accept", "application/json".into()));
                let reply = idp(request).await?;
                Event::UserInfo { status: reply.status, body: String::from_utf8_lossy(&reply.body).into_owned() }
            }
            Some(Effect::SignIn(identity)) => {
                let user = users.store.oidc_user(&identity.subject, &identity.name).map_err(|_| OidcError::UserInfo("the user store failed"))?;
                let session = users.signed_in(&user, shared.now());
                let mut response = see_other(shared.origin.as_deref().unwrap_or("/"));
                for cookie in session.headers().get_all(header::SET_COOKIE) { response.headers_mut().append(header::SET_COOKIE, cookie.clone()); }
                return Ok(response);
            }
            Some(Effect::Fail(e)) => return Err(e),
            None => return Err(OidcError::WrongState),
        };
    }
}

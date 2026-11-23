//! The browser side of the Relay API (architecture §8): a [`Transport`] over `fetch`, the
//! `/v1/events` WebSocket for push, and the Relay sign-in calls.
//!
//! Two ways to be signed in to a Relay ([`Auth`]):
//! - **Users** (a multi-user Relay): `POST /v1/auth/login` sets an `HttpOnly` session cookie and
//!   answers `{csrf, user}`. Requests go with `credentials: "include"`, and every request that is
//!   not a `GET` carries the CSRF value in `x-ravenglass-csrf`. The browser holds the cookie; this
//!   code never sees it.
//! - **Token** (a self-hosted, single-user Relay): `Authorization: Bearer <token>` on every
//!   request, no cookies (`credentials: "omit"`), and `?access_token=` on the WebSocket, which
//!   cannot carry headers. Where the token is kept is the app's choice (session storage, never
//!   local storage).
//!
//! Futures built from JavaScript promises are not `Send`, but [`mail_core::BoxFut`] is. On
//! `wasm32-unknown-unknown` without the `atomics` target feature there is exactly one thread, so
//! [`Local`] asserts `Send` for them; a threaded wasm build refuses to compile this file instead.

#[cfg(target_feature = "atomics")]
compile_error!("relay-client's web transport assumes single-threaded wasm (no `atomics`)");

use crate::{HttpResponse, RelayBackend, Transport};
use mail_core::*;
use relay_proto::*;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::task::{Context, Poll, Waker};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, MessageEvent, Request, RequestCredentials, RequestInit, RequestMode, Response, WebSocket};

/// The header that carries the CSRF value of a cookie session.
pub const CSRF_HEADER: &str = "x-ravenglass-csrf";
pub const LOGIN: &str = "/v1/auth/login";
pub const SESSION: &str = "/v1/auth/session";
pub const LOGOUT: &str = "/v1/auth/logout";

/// How requests prove who is asking.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    /// A cookie session from [`login`] or [`session`]; `csrf` goes on every non-`GET` request.
    Session { csrf: String },
    /// The single-user Relay's shared bearer token.
    Token(String),
}

impl std::fmt::Debug for Auth {
    /// Never prints a secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Auth::Session { .. } => "Auth::Session", Auth::Token(_) => "Auth::Token" })
    }
}

/// What `/v1/auth/session` and `/v1/auth/login` answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn { pub csrf: String, pub user: String }

#[derive(Deserialize)]
struct SessionBody { csrf: String, #[serde(default)] user: serde_json::Value }

/// Why a sign-in call did not give a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInError {
    /// 401: not signed in, or wrong username or password.
    Unauthorized,
    /// The Relay could not be reached (offline, CORS refused, wrong address).
    Network(String),
    /// Any other answer.
    Server(String),
}

impl std::fmt::Display for SignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignInError::Unauthorized => f.write_str("not signed in"),
            SignInError::Network(m) => write!(f, "the relay cannot be reached: {m}"),
            SignInError::Server(m) => write!(f, "the relay refused: {m}"),
        }
    }
}

/// `fetch` to one Relay origin.
#[derive(Clone, Debug)]
pub struct WebTransport { base: String, auth: Auth }

impl WebTransport {
    /// `base` is the Relay's origin, e.g. `https://relay.example.org`; empty means this page's.
    pub fn new(base: &str, auth: Auth) -> Self { Self { base: base.trim_end_matches('/').to_owned(), auth } }

    pub fn auth(&self) -> &Auth { &self.auth }

    /// The push socket's URL: `ws(s)://<relay>/v1/events`, with the token in the query in token mode.
    pub fn events_url(&self) -> String {
        let base = if self.base.is_empty() { page_origin() } else { self.base.clone() };
        let ws = base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
        match &self.auth {
            Auth::Token(token) => format!("{ws}{}?access_token={}", paths::EVENTS, segment(token)),
            Auth::Session { .. } => format!("{ws}{}", paths::EVENTS),
        }
    }
}

impl Transport for WebTransport {
    fn request<'a>(&'a self, version: u32, method: Method, path: &'a str, body: Vec<u8>) -> BoxFut<'a, Result<HttpResponse>> {
        let url = format!("{}{path}", self.base);
        let verb = match method { Method::Get => "GET", Method::Post => "POST" };
        Box::pin(Local(async move {
            let mut headers = vec![(VERSION_HEADER, version.to_string())];
            if method == Method::Post { headers.push(("content-type", "application/json".to_owned())); }
            let (credentials, extra) = credentials(&self.auth, method);
            headers.extend(extra);
            let body = (method == Method::Post).then_some(body);
            fetch(&url, verb, &headers, body, credentials).await.map_err(MailError::Network)
        }))
    }
}

/// Cookies for a session, the bearer header for a token.
fn credentials(auth: &Auth, method: Method) -> (RequestCredentials, Vec<(&'static str, String)>) {
    match auth {
        Auth::Session { csrf } => (RequestCredentials::Include, if method == Method::Get { Vec::new() } else { vec![(CSRF_HEADER, csrf.clone())] }),
        Auth::Token(token) => (RequestCredentials::Omit, vec![("authorization", format!("Bearer {token}"))]),
    }
}

/// Is there a cookie session? `Unauthorized` means show the sign-in screen.
///
/// A token-only Relay does not allow credentialed requests from the page, so the browser hides
/// its answer and the fetch fails as if offline. Asking again without credentials tells the
/// two apart: a Relay that answers at all is reachable, and has no session for this tab.
pub async fn session(base: &str) -> std::result::Result<SignedIn, SignInError> {
    let url = format!("{}{SESSION}", base.trim_end_matches('/'));
    match fetch(&url, "GET", &[], None, RequestCredentials::Include).await {
        Err(offline) => match fetch(&url, "GET", &[], None, RequestCredentials::Omit).await {
            Ok(_) => Err(SignInError::Unauthorized),
            Err(_) => Err(SignInError::Network(offline)),
        },
        answer => signed_in(answer),
    }
}

/// Sign in to a multi-user Relay. The password goes in the request body and nowhere else.
pub async fn login(base: &str, username: &str, password: &str) -> std::result::Result<SignedIn, SignInError> {
    let url = format!("{}{LOGIN}", base.trim_end_matches('/'));
    let body = serde_json::to_vec(&serde_json::json!({ "username": username, "password": password })).map_err(|e| SignInError::Server(e.to_string()))?;
    signed_in(fetch(&url, "POST", &[("content-type", "application/json".to_owned())], Some(body), RequestCredentials::Include).await)
}

/// End a cookie session. Best effort: the cookie is the Relay's to clear.
pub async fn logout(base: &str, csrf: &str) -> std::result::Result<(), SignInError> {
    let url = format!("{}{LOGOUT}", base.trim_end_matches('/'));
    let answer = fetch(&url, "POST", &[(CSRF_HEADER, csrf.to_owned())], Some(Vec::new()), RequestCredentials::Include).await.map_err(SignInError::Network)?;
    if (200..300).contains(&answer.status) { Ok(()) } else { Err(SignInError::Server(format!("HTTP {}", answer.status))) }
}

/// Check a bearer token by opening a protocol session: the accounts it may use, or why not.
pub async fn check_token(base: &str, token: &str) -> std::result::Result<SessionOpened, SignInError> {
    let probe = RelayBackend::new(AccountId::from(""), WebTransport::new(base, Auth::Token(token.to_owned())));
    match probe.open_session().await {
        Ok(opened) => Ok(opened),
        Err(MailError::Auth) => Err(SignInError::Unauthorized),
        Err(MailError::Network(m)) => Err(SignInError::Network(m)),
        Err(other) => Err(SignInError::Server(other.to_string())),
    }
}

fn signed_in(answer: std::result::Result<HttpResponse, String>) -> std::result::Result<SignedIn, SignInError> {
    let answer = answer.map_err(SignInError::Network)?;
    match answer.status {
        200..=299 => {
            let body: SessionBody = serde_json::from_slice(&answer.body).map_err(|e| SignInError::Server(format!("unreadable answer: {e}")))?;
            Ok(SignedIn { csrf: body.csrf, user: user_name(&body.user) })
        }
        401 | 403 => Err(SignInError::Unauthorized),
        status => Err(SignInError::Server(format!("HTTP {status}"))),
    }
}

/// The user as one line: a string, or an object's `username`, `name` or `email`.
fn user_name(user: &serde_json::Value) -> String {
    match user {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) => ["username", "name", "email"].iter().find_map(|k| o.get(*k).and_then(|v| v.as_str())).unwrap_or("").to_owned(),
        _ => String::new(),
    }
}

/// One `fetch`, its status and whole body. `Err` is a network failure (or CORS refusal).
async fn fetch(url: &str, method: &str, headers: &[(&str, String)], body: Option<Vec<u8>>, credentials: RequestCredentials) -> std::result::Result<HttpResponse, String> {
    let init = RequestInit::new();
    init.set_method(method);
    init.set_mode(RequestMode::Cors);
    init.set_credentials(credentials);
    let list = Headers::new().map_err(js_text)?;
    for (name, value) in headers { list.set(name, value).map_err(js_text)?; }
    init.set_headers(&list);
    if let Some(body) = &body { init.set_body(&js_sys::Uint8Array::from(body.as_slice())); }
    let request = Request::new_with_str_and_init(url, &init).map_err(js_text)?;
    let fetch = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("fetch")).map_err(js_text)?.dyn_into::<js_sys::Function>().map_err(js_text)?;
    let promise = fetch.call1(&js_sys::global(), &request).map_err(js_text)?.dyn_into::<js_sys::Promise>().map_err(js_text)?;
    let response: Response = JsFuture::from(promise).await.map_err(js_text)?.dyn_into().map_err(js_text)?;
    let status = response.status();
    let buffer = JsFuture::from(response.array_buffer().map_err(js_text)?).await.map_err(js_text)?;
    Ok(HttpResponse { status, body: js_sys::Uint8Array::new(&buffer).to_vec() })
}

fn js_text(value: JsValue) -> String {
    value.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message())).or_else(|| value.as_string()).unwrap_or_else(|| "a browser error".into())
}

fn page_origin() -> String {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("location"))
        .and_then(|l| js_sys::Reflect::get(&l, &JsValue::from_str("origin")))
        .ok()
        .and_then(|o| o.as_string())
        .unwrap_or_default()
}

/// [`RelayBackend`] over `fetch`, plus push: [`MailBackend::idle`] holds the `/v1/events`
/// WebSocket and turns each push into an [`IdleEvent`]. The socket carries every account's
/// pushes, so each one wakes a refresh of this account; the refresh finds nothing new for the
/// others.
pub struct WebBackend { inner: RelayBackend<WebTransport>, events: String }

impl WebBackend {
    pub fn new(account: AccountId, transport: WebTransport) -> Self {
        let events = transport.events_url();
        Self { inner: RelayBackend::new(account, transport), events }
    }
}

impl MailBackend for WebBackend {
    fn account(&self) -> &AccountId { self.inner.account() }
    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> { self.inner.list_folders() }
    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> { self.inner.sync_folder(folder, since) }
    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> { self.inner.fetch_body(msg) }
    fn set_flags<'a>(&'a self, msgs: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>> { self.inner.set_flags(msgs, add, remove) }
    fn move_to<'a>(&'a self, msgs: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>> { self.inner.move_to(msgs, to) }
    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> { self.inner.append_draft(raw) }
    fn send<'a>(&'a self, raw: &'a [u8], from: &'a Address, rcpt: &'a [Address]) -> BoxFut<'a, Result<()>> { self.inner.send(raw, from, rcpt) }
    fn idle<'a>(&'a self, _folder: &'a FolderId, events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> {
        Box::pin(Local(async move { Socket::open(&self.events)?.forward(events).await }))
    }
}

/// What the socket's callbacks hand the future.
#[derive(Default)]
struct Inbox { texts: VecDeque<String>, closed: bool, waker: Option<Waker> }

/// An open `/v1/events` socket. Dropping it closes the socket.
struct Socket {
    ws: WebSocket,
    inbox: Rc<RefCell<Inbox>>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut(JsValue)>,
}

impl Socket {
    fn open(url: &str) -> Result<Socket> {
        let ws = WebSocket::new(url).map_err(|e| MailError::Network(js_text(e)))?;
        let inbox = Rc::new(RefCell::new(Inbox::default()));
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new({
            let inbox = inbox.clone();
            move |event: MessageEvent| {
                let Some(text) = event.data().as_string() else { return };
                let mut inbox = inbox.borrow_mut();
                inbox.texts.push_back(text);
                if let Some(waker) = inbox.waker.take() { waker.wake(); }
            }
        });
        let on_close = Closure::<dyn FnMut(JsValue)>::new({
            let inbox = inbox.clone();
            move |_: JsValue| {
                let mut inbox = inbox.borrow_mut();
                inbox.closed = true;
                if let Some(waker) = inbox.waker.take() { waker.wake(); }
            }
        });
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        // An error is always followed by close; close is what ends the watch.
        Ok(Socket { ws, inbox, _on_message: on_message, _on_close: on_close })
    }

    /// Pushes become idle events until the socket closes.
    fn forward(self, events: Sender<IdleEvent>) -> Pin<Box<dyn Future<Output = Result<()>>>> {
        Box::pin(std::future::poll_fn(move |cx: &mut Context<'_>| {
            let mut inbox = self.inbox.borrow_mut();
            while let Some(text) = inbox.texts.pop_front() {
                let event = match serde_json::from_str::<Push>(&text) {
                    Ok(Push::Exists { folder }) => IdleEvent::Exists(folder),
                    Ok(Push::Flags { folder }) => IdleEvent::FlagsChanged(folder),
                    Ok(Push::Expunge { folder }) => IdleEvent::Expunged(folder),
                    Ok(Push::SyncState { .. }) | Err(_) => continue,
                };
                if events.send(event).is_err() { return Poll::Ready(Ok(())); }
            }
            if inbox.closed { return Poll::Ready(Err(MailError::Network("the relay's push connection closed".into()))); }
            inbox.waker = Some(cx.waker().clone());
            Poll::Pending
        }))
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.ws.set_onmessage(None);
        self.ws.set_onclose(None);
        let _ = self.ws.close();
    }
}

/// A future that is `Send` because the only thread there is cannot hand it to another one.
struct Local<F>(F);

// SAFETY: this file only compiles for single-threaded wasm (see the `compile_error!` above), where
// no value can reach a second thread, so the `Send` bound on `BoxFut` cannot be relied on to cross one.
unsafe impl<F> Send for Local<F> {}

impl<F: Future> Future for Local<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        // SAFETY: structural pinning of the only field; `Local` never moves it out.
        unsafe { self.map_unchecked_mut(|local| &mut local.0) }.poll(cx)
    }
}

//! The Relay over HTTP (architecture §8). Every request must carry the shared token; the
//! WebSocket at `/v1/events` pushes folder changes and carries no message content.

use crate::Relay;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, HeaderValue, Method as HttpMethod, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use mail_core::{FolderRole, IdleEvent, MailBackend, block_on};
use relay_proto::{ApiError, ErrorCode, Method, Push, VERSION_HEADER, paths};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Largest request body: a 50 MB message plus JSON overhead.
const MAX_BODY: usize = 96 << 20;
/// How long a push watcher waits before reconnecting after its connection ends.
const REWATCH: std::time::Duration = std::time::Duration::from_secs(10);

pub struct Config {
    /// The secret every request presents as `Authorization: Bearer …`. At least 16 characters.
    pub token: String,
    /// The web app's origin, the only one allowed to call from a browser. `None`: no browser access.
    pub origin: Option<String>,
}

#[derive(Clone)]
struct Shared { relay: Arc<Relay>, config: Arc<Config>, pushes: broadcast::Sender<String> }

/// The router. Starts one push watcher thread per account.
pub fn router(relay: Relay, config: Config) -> Result<Router, &'static str> {
    if config.token.len() < 16 { return Err("the relay token must be at least 16 characters"); }
    let (pushes, _) = broadcast::channel(256);
    for backend in relay.backends() { watch(backend.clone(), pushes.clone()); }
    let shared = Shared { relay: Arc::new(relay), config: Arc::new(config), pushes };
    Ok(Router::new().route(paths::EVENTS, get(events)).fallback(api).layer(DefaultBodyLimit::max(MAX_BODY)).with_state(shared))
}

/// Serve until the listener fails.
pub async fn serve(listener: tokio::net::TcpListener, relay: Relay, config: Config) -> std::io::Result<()> {
    let router = router(relay, config).map_err(std::io::Error::other)?;
    axum::serve(listener, router).await
}

/// Compare without stopping at the first difference, so timing does not reveal the token.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn authorized(shared: &Shared, headers: &HeaderMap) -> bool {
    let presented = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    presented.is_some_and(|t| same(t, &shared.config.token))
}

fn error(code: ErrorCode, message: &str) -> Response {
    let body = serde_json::to_vec(&ApiError::new(code, message)).unwrap_or_default();
    (StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// Let the configured origin, and only it, read responses from a browser.
fn with_cors(shared: &Shared, headers: &HeaderMap, mut response: Response) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if let (Some(allowed), Some(origin)) = (&shared.config.origin, origin)
        && allowed == origin
        && let Ok(value) = HeaderValue::from_str(origin)
    {
        let h = response.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        h.insert(header::VARY, HeaderValue::from_static("origin"));
        h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("authorization, content-type, ravenglass-relay-version"));
        h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST"));
    }
    response
}

async fn api(State(shared): State<Shared>, method: HttpMethod, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    // A preflight carries no credentials; it only learns whether its origin is allowed.
    if method == HttpMethod::OPTIONS { return with_cors(&shared, &headers, StatusCode::NO_CONTENT.into_response()); }
    let response = if !authorized(&shared, &headers) {
        error(ErrorCode::Auth, "missing or wrong relay token")
    } else {
        let version = headers.get(VERSION_HEADER).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
        let method = match method { HttpMethod::GET => Some(Method::Get), HttpMethod::POST => Some(Method::Post), _ => None };
        match method {
            Some(method) => {
                let answer = shared.relay.handle(version, method, uri.path(), &body).await;
                (StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), [(header::CONTENT_TYPE, answer.content_type)], answer.body).into_response()
            }
            None => error(ErrorCode::NotFound, "no such route"),
        }
    };
    with_cors(&shared, &headers, response)
}

/// Browsers cannot set headers on a WebSocket, so the token may come as `?access_token=`.
async fn events(State(shared): State<Shared>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>, upgrade: WebSocketUpgrade) -> Response {
    let by_query = query.get("access_token").is_some_and(|t| same(t, &shared.config.token));
    if !(by_query || authorized(&shared, &headers)) { return error(ErrorCode::Auth, "missing or wrong relay token"); }
    let pushes = shared.pushes.subscribe();
    upgrade.on_upgrade(move |socket| push_loop(socket, pushes))
}

async fn push_loop(mut socket: WebSocket, mut pushes: broadcast::Receiver<String>) {
    loop {
        tokio::select! {
            pushed = pushes.recv() => match pushed {
                Ok(json) => { if socket.send(Message::Text(json.into())).await.is_err() { break; } }
                // A slow client missed some pushes; it re-syncs on the next one anyway.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => if !matches!(incoming, Some(Ok(_))) { break },
        }
    }
}

/// Hold the account's push connection for as long as the server runs.
fn watch(backend: Arc<dyn MailBackend>, pushes: broadcast::Sender<String>) {
    let _ = std::thread::Builder::new().name("relay-push".into()).spawn(move || loop {
        let inbox = block_on(backend.list_folders()).ok().and_then(|f| f.into_iter().find(|f| f.role == FolderRole::Inbox));
        if let Some(inbox) = inbox {
            let (pushed, events) = std::sync::mpsc::channel::<IdleEvent>();
            let forward = std::thread::spawn({
                let pushes = pushes.clone();
                move || for event in events { if let Ok(json) = serde_json::to_string(&Push::from(event)) { let _ = pushes.send(json); } }
            });
            let _ = block_on(backend.idle(&inbox.id, pushed));
            let _ = forward.join();
        }
        std::thread::sleep(REWATCH);
    });
}

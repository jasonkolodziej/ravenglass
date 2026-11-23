//! OIDC sign-in against an in-process fake identity provider: the whole browser round trip,
//! and the ways it must fail.

mod common;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine;
use common::*;
use ravenglass_relay::oidc::Client;
use ravenglass_relay::server::OidcConfig;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// What the fake identity provider does wrong, if anything.
#[derive(Clone, Default)]
struct Behaviour { wrong_nonce: bool, email: Option<String> }

#[derive(Clone)]
struct Idp { issuer: String, behaviour: Arc<Mutex<Behaviour>>, codes: Arc<Mutex<HashMap<String, (String, String)>>> }

fn b64(bytes: &[u8]) -> String { base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes) }

fn json_reply(value: serde_json::Value) -> axum::response::Response { ([(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response() }

async fn discovery(State(idp): State<Idp>) -> impl IntoResponse {
    json_reply(serde_json::json!({
        "issuer": idp.issuer,
        "authorization_endpoint": format!("{}/authorize", idp.issuer),
        "token_endpoint": format!("{}/token", idp.issuer),
        "userinfo_endpoint": format!("{}/userinfo", idp.issuer),
    }))
}

/// The user "signs in" at once; the provider sends the browser back with a code.
async fn authorize(State(idp): State<Idp>, Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["code_challenge_method"], "S256");
    let code = format!("code-{}", idp.codes.lock().unwrap().len());
    idp.codes.lock().unwrap().insert(code.clone(), (q["nonce"].clone(), q["code_challenge"].clone()));
    let location = format!("{}?code={code}&state={}", q["redirect_uri"], q["state"]);
    (StatusCode::FOUND, [(header::LOCATION, location)])
}

async fn token(State(idp): State<Idp>, headers: HeaderMap, body: String) -> impl IntoResponse {
    let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
    let expected_basic = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("relay-client:idp-secret"));
    if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some(expected_basic.as_str()) { return (StatusCode::UNAUTHORIZED, "client auth").into_response(); }
    let Some((nonce, challenge)) = idp.codes.lock().unwrap().remove(&form["code"]) else { return (StatusCode::BAD_REQUEST, "invalid_grant").into_response() };
    use sha2::Digest;
    if b64(&sha2::Sha256::digest(form["code_verifier"].as_bytes())) != challenge { return (StatusCode::BAD_REQUEST, "pkce").into_response() }
    let behaviour = idp.behaviour.lock().unwrap().clone();
    let nonce = if behaviour.wrong_nonce { "some-other-nonce".to_owned() } else { nonce };
    // The Relay under test runs on its own clock, which starts here.
    let now = 1_800_000_000u64;
    let claims = serde_json::json!({ "iss": idp.issuer, "sub": "user-123", "aud": "relay-client", "exp": now + 300, "iat": now, "nonce": nonce });
    let id_token = format!("{}.{}.{}", b64(br#"{"alg":"RS256","typ":"JWT"}"#), b64(claims.to_string().as_bytes()), b64(b"signature"));
    json_reply(serde_json::json!({ "access_token": "idp-access-token", "token_type": "Bearer", "expires_in": 300, "id_token": id_token })).into_response()
}

async fn userinfo(State(idp): State<Idp>, headers: HeaderMap) -> impl IntoResponse {
    if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some("Bearer idp-access-token") { return StatusCode::UNAUTHORIZED.into_response(); }
    let email = idp.behaviour.lock().unwrap().email.clone().unwrap_or_else(|| "carol@example.org".into());
    json_reply(serde_json::json!({ "sub": "user-123", "email": email, "email_verified": true })).into_response()
}

fn start_idp() -> Idp {
    let (ready, issuer) = std::sync::mpsc::channel();
    let behaviour = Arc::new(Mutex::new(Behaviour::default()));
    let shared = behaviour.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let idp = Idp { issuer: format!("http://{}", listener.local_addr().unwrap()), behaviour: shared, codes: Arc::default() };
            ready.send(idp.issuer.clone()).unwrap();
            let app = axum::Router::new()
                .route("/.well-known/openid-configuration", get(discovery))
                .route("/authorize", get(authorize))
                .route("/token", post(token))
                .route("/userinfo", get(userinfo))
                .with_state(idp);
            axum::serve(listener, app).await.unwrap();
        });
    });
    Idp { issuer: issuer.recv().unwrap(), behaviour, codes: Arc::default() }
}

fn relay_with(idp: &Idp, allowed_domains: &[&str]) -> Relay {
    let issuer = idp.issuer.clone();
    let allowed_domains: Vec<String> = allowed_domains.iter().map(|d| d.to_string()).collect();
    start(move |config, base| {
        config.oidc = Some(OidcConfig {
            issuer,
            client: Client {
                client_id: "relay-client".into(),
                client_secret: Some("idp-secret".into()),
                redirect_uri: format!("{base}/v1/auth/oidc/callback"),
                scopes: "openid email".into(),
                allowed_emails: vec![],
                allowed_domains,
            },
        });
    })
}

/// The browser's trip: start at the Relay, sign in at the provider, come back. Returns the
/// callback URL and the flow cookie, so tests can tamper with either.
fn go_to_idp(relay: &Relay) -> (String, String) {
    let start = call("GET", &format!("{}/v1/auth/oidc/start", relay.base), &[], None);
    assert_eq!(start.status, 303, "{}", start.text());
    let flow_cookie = start.header("set-cookie").unwrap().to_owned();
    assert!(flow_cookie.contains("HttpOnly") && flow_cookie.contains("SameSite=Lax") && flow_cookie.contains("Secure"), "{flow_cookie}");
    let at_idp = call("GET", start.header("location").unwrap(), &[], None);
    assert_eq!(at_idp.status, 302);
    (at_idp.header("location").unwrap().to_owned(), flow_cookie.split(';').next().unwrap().to_owned())
}

#[test]
fn signing_in_with_oidc_creates_a_session_and_returns_to_the_app() {
    let idp = start_idp();
    let relay = relay_with(&idp, &["example.org"]);
    assert_eq!(call("GET", &format!("{}/v1/auth/config", relay.base), &[], None).json()["oidc"], true);
    let (callback, flow) = go_to_idp(&relay);
    let back = call("GET", &callback, &[("cookie", &flow)], None);
    assert_eq!(back.status, 303, "{}", back.text());
    assert_eq!(back.header("location"), Some(ORIGIN));
    let session_cookie = back.all("set-cookie").into_iter().find(|c| c.starts_with("__Host-ravenglass-session=")).unwrap().to_owned();
    assert!(session_cookie.contains("SameSite=Strict") && session_cookie.contains("HttpOnly"));
    let cookie = session_cookie.split(';').next().unwrap();
    let session = call("GET", &format!("{}/v1/auth/session", relay.base), &[("cookie", cookie)], None);
    assert_eq!(session.status, 200);
    assert_eq!(session.json()["user"]["name"], "carol@example.org");

    let again = call("GET", &callback, &[("cookie", &flow)], None);
    assert_eq!(again.status, 400, "a callback works once");
    let (callback, flow) = go_to_idp(&relay);
    let second = call("GET", &callback, &[("cookie", &flow)], None);
    assert_eq!(second.status, 303);
    let users = relay.store.users().unwrap();
    assert_eq!(users.iter().filter(|u| u.name == "carol@example.org").count(), 1, "the same subject is the same user");
}

#[test]
fn a_wrong_state_or_a_missing_flow_cookie_is_refused() {
    let idp = start_idp();
    let relay = relay_with(&idp, &[]);
    let (callback, flow) = go_to_idp(&relay);
    let forged = callback.split("&state=").next().unwrap().to_owned() + "&state=forged-state";
    let answer = call("GET", &forged, &[("cookie", &flow)], None);
    assert_eq!(answer.status, 400);
    assert!(answer.all("set-cookie").iter().all(|c| !c.starts_with("__Host-ravenglass-session=")));
    let (callback, _) = go_to_idp(&relay);
    assert_eq!(call("GET", &callback, &[], None).status, 400, "another browser cannot finish this sign-in");
    let (callback, _) = go_to_idp(&relay);
    assert_eq!(call("GET", &callback, &[("cookie", "__Secure-ravenglass-oidc=someone-elses")], None).status, 400);
}

#[test]
fn an_id_token_with_the_wrong_nonce_is_refused() {
    let idp = start_idp();
    let relay = relay_with(&idp, &[]);
    idp.behaviour.lock().unwrap().wrong_nonce = true;
    let (callback, flow) = go_to_idp(&relay);
    let answer = call("GET", &callback, &[("cookie", &flow)], None);
    assert_eq!(answer.status, 400);
    assert!(answer.text().contains("not for this sign-in"), "{}", answer.text());
    assert!(relay.store.users().unwrap().iter().all(|u| u.name != "carol@example.org"));
}

#[test]
fn an_email_outside_the_allowlist_is_refused() {
    let idp = start_idp();
    let relay = relay_with(&idp, &["example.org"]);
    idp.behaviour.lock().unwrap().email = Some("eve@elsewhere.net".into());
    let (callback, flow) = go_to_idp(&relay);
    let answer = call("GET", &callback, &[("cookie", &flow)], None);
    assert_eq!(answer.status, 403);
    assert!(answer.all("set-cookie").iter().all(|c| !c.starts_with("__Host-ravenglass-session=")));
    assert_eq!(relay.store.users().unwrap().len(), 2, "no user was created");
}

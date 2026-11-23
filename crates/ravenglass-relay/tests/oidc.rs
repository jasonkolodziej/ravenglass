//! OIDC sign-in against an in-process fake identity provider that publishes a JWK Set and signs
//! its ID tokens: the whole browser round trip, and the ways it must fail.

mod common;
#[path = "oidc/keys.rs"]
mod keys;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use common::*;
use keys::{b64, hmac_sha256, keys, signing_input};
use ravenglass_relay::oidc::Client;
use ravenglass_relay::server::OidcConfig;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// How the fake identity provider makes its ID token.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Forgery {
    /// Signed properly with the key for `Behaviour::alg`.
    #[default]
    None,
    /// Signed, then a claim added to the payload.
    TamperPayload,
    /// Signed, then one bit of the signature flipped.
    TamperSignature,
    /// `alg: none` and no signature.
    AlgNone,
    /// `alg: HS256`, keyed with the provider's published RSA key.
    Hs256WithPublicKey,
    /// Signed with a key whose `kid` is not published.
    UnknownKid,
    /// Signed with the rotated key (`kid: rot`), published only once `Behaviour::rotated` is set.
    Rotated,
    /// `alg: ES256` naming the RSA key, signed with the EC key.
    AlgMismatch,
    /// Signed by an attacker who puts their key in `jwk` and points `jku`/`x5u` at it.
    CarriedKey,
    /// Not signed at all (`"signature"`), as before signature checks.
    Unsigned,
}

/// What the fake identity provider does, and does wrong.
#[derive(Clone)]
struct Behaviour {
    wrong_nonce: bool,
    email: Option<String>,
    alg: &'static str,
    forgery: Forgery,
    /// The ID token's `iat`; tests move it with the Relay's clock.
    issued_at: u64,
    /// Publish the rotated key in the JWKS.
    rotated: bool,
    /// `Cache-Control: max-age` on the JWKS.
    jwks_max_age: u64,
    /// Name a `jwks_uri` in discovery.
    jwks_uri: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self { wrong_nonce: false, email: None, alg: "RS256", forgery: Forgery::None, issued_at: 1_800_000_000, rotated: false, jwks_max_age: 3600, jwks_uri: true }
    }
}

#[derive(Clone)]
struct Idp {
    issuer: String,
    behaviour: Arc<Mutex<Behaviour>>,
    codes: Arc<Mutex<HashMap<String, (String, String)>>>,
    /// JWKS fetches, and fetches of the attacker's `jku`.
    jwks_fetches: Arc<AtomicUsize>,
    evil_fetches: Arc<AtomicUsize>,
}

impl Idp {
    fn set(&self, change: impl FnOnce(&mut Behaviour)) { change(&mut self.behaviour.lock().unwrap()); }
    fn jwks_fetches(&self) -> usize { self.jwks_fetches.load(Ordering::SeqCst) }
}

fn json_reply(value: serde_json::Value) -> axum::response::Response { ([(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response() }

async fn discovery(State(idp): State<Idp>) -> impl IntoResponse {
    let mut doc = serde_json::json!({
        "issuer": idp.issuer,
        "authorization_endpoint": format!("{}/authorize", idp.issuer),
        "token_endpoint": format!("{}/token", idp.issuer),
        "userinfo_endpoint": format!("{}/userinfo", idp.issuer),
    });
    if idp.behaviour.lock().unwrap().jwks_uri { doc["jwks_uri"] = format!("{}/jwks", idp.issuer).into(); }
    json_reply(doc)
}

async fn jwks(State(idp): State<Idp>) -> impl IntoResponse {
    idp.jwks_fetches.fetch_add(1, Ordering::SeqCst);
    let behaviour = idp.behaviour.lock().unwrap().clone();
    let k = keys();
    let mut published = vec![k.rsa.jwk("rsa"), k.ec.jwk("ec"), k.ed.jwk("ed")];
    if behaviour.rotated { published.push(k.rotated.jwk("rot")); }
    let cache = format!("public, max-age={}", behaviour.jwks_max_age);
    ([(header::CONTENT_TYPE, "application/jwk-set+json".to_owned()), (header::CACHE_CONTROL, cache)], serde_json::json!({ "keys": published }).to_string())
}

/// The attacker's key set, named by `jku`. The Relay must never come here.
async fn evil_jwks(State(idp): State<Idp>) -> impl IntoResponse {
    idp.evil_fetches.fetch_add(1, Ordering::SeqCst);
    json_reply(serde_json::json!({ "keys": [keys().attacker.jwk("ed")] }))
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

/// The ID token, made the way `behaviour` says.
fn id_token(issuer: &str, behaviour: &Behaviour, claims: serde_json::Value) -> String {
    let k = keys();
    let alg = behaviour.alg;
    let key = match alg { "RS256" | "PS256" => &k.rsa, "ES256" => &k.ec, _ => &k.ed };
    let kid = match alg { "RS256" | "PS256" => "rsa", "ES256" => "ec", _ => "ed" };
    let signed = |header: serde_json::Value, key: &keys::Key, alg: &str| {
        let input = signing_input(&header, &claims);
        let signature = key.sign(alg, input.as_bytes());
        format!("{input}.{}", b64(&signature))
    };
    match behaviour.forgery {
        Forgery::None => signed(serde_json::json!({ "alg": alg, "kid": kid, "typ": "JWT" }), key, alg),
        Forgery::TamperPayload => {
            let token = signed(serde_json::json!({ "alg": alg, "kid": kid }), key, alg);
            let parts: Vec<&str> = token.split('.').collect();
            let mut tampered = claims.clone();
            tampered["email"] = "boss@example.org".into();
            format!("{}.{}.{}", parts[0], b64(tampered.to_string().as_bytes()), parts[2])
        }
        Forgery::TamperSignature => {
            let input = signing_input(&serde_json::json!({ "alg": alg, "kid": kid }), &claims);
            let mut signature = key.sign(alg, input.as_bytes());
            signature[7] ^= 0x01;
            format!("{input}.{}", b64(&signature))
        }
        Forgery::AlgNone => format!("{}.", signing_input(&serde_json::json!({ "alg": "none", "kid": "rsa" }), &claims)),
        Forgery::Hs256WithPublicKey => {
            let input = signing_input(&serde_json::json!({ "alg": "HS256", "kid": "rsa" }), &claims);
            let secret = k.rsa.jwk("rsa").to_string();
            format!("{input}.{}", b64(&hmac_sha256(secret.as_bytes(), input.as_bytes())))
        }
        Forgery::UnknownKid => signed(serde_json::json!({ "alg": "EdDSA", "kid": "not-published" }), &k.rotated, "EdDSA"),
        Forgery::Rotated => signed(serde_json::json!({ "alg": "EdDSA", "kid": "rot" }), &k.rotated, "EdDSA"),
        Forgery::AlgMismatch => signed(serde_json::json!({ "alg": "ES256", "kid": "rsa" }), &k.ec, "ES256"),
        Forgery::CarriedKey => {
            let header = serde_json::json!({ "alg": "EdDSA", "kid": "ed", "jwk": k.attacker.jwk("ed"), "jku": format!("{issuer}/evil-jwks"), "x5u": format!("{issuer}/evil-cert") });
            signed(header, &k.attacker, "EdDSA")
        }
        Forgery::Unsigned => format!("{}.{}", signing_input(&serde_json::json!({ "alg": "RS256", "typ": "JWT" }), &claims), b64(b"signature")),
    }
}

async fn token(State(idp): State<Idp>, headers: HeaderMap, body: String) -> impl IntoResponse {
    let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes()).into_owned().collect();
    let expected_basic = format!("Basic {}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "relay-client:idp-secret"));
    if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some(expected_basic.as_str()) { return (StatusCode::UNAUTHORIZED, "client auth").into_response(); }
    let Some((nonce, challenge)) = idp.codes.lock().unwrap().remove(&form["code"]) else { return (StatusCode::BAD_REQUEST, "invalid_grant").into_response() };
    use sha2::Digest;
    if b64(&sha2::Sha256::digest(form["code_verifier"].as_bytes())) != challenge { return (StatusCode::BAD_REQUEST, "pkce").into_response() }
    let behaviour = idp.behaviour.lock().unwrap().clone();
    let nonce = if behaviour.wrong_nonce { "some-other-nonce".to_owned() } else { nonce };
    let now = behaviour.issued_at;
    let claims = serde_json::json!({ "iss": idp.issuer, "sub": "user-123", "aud": "relay-client", "exp": now + 300, "iat": now, "nonce": nonce });
    let id_token = id_token(&idp.issuer, &behaviour, claims);
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
    let (jwks_fetches, evil_fetches) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let shared = Idp { issuer: String::new(), behaviour: behaviour.clone(), codes: Arc::default(), jwks_fetches: jwks_fetches.clone(), evil_fetches: evil_fetches.clone() };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let idp = Idp { issuer: format!("http://{}", listener.local_addr().unwrap()), ..shared };
            ready.send(idp.issuer.clone()).unwrap();
            let app = axum::Router::new()
                .route("/.well-known/openid-configuration", get(discovery))
                .route("/jwks", get(jwks))
                .route("/evil-jwks", get(evil_jwks))
                .route("/authorize", get(authorize))
                .route("/token", post(token))
                .route("/userinfo", get(userinfo))
                .with_state(idp);
            axum::serve(listener, app).await.unwrap();
        });
    });
    Idp { issuer: issuer.recv().unwrap(), behaviour, codes: Arc::default(), jwks_fetches, evil_fetches }
}

fn relay_with(idp: &Idp, allowed_domains: &[&str]) -> Relay { relay_checking(idp, allowed_domains, true) }

fn relay_checking(idp: &Idp, allowed_domains: &[&str], verify_signatures: bool) -> Relay {
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
            verify_signatures,
        });
    })
}

/// Move the Relay's clock and the provider's together.
fn advance(relay: &Relay, idp: &Idp, seconds: u64) {
    relay.advance(seconds);
    idp.set(|b| b.issued_at += seconds);
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

/// One whole sign-in: the callback's answer.
fn sign_in_once(relay: &Relay) -> Answer {
    let (callback, flow) = go_to_idp(relay);
    call("GET", &callback, &[("cookie", &flow)], None)
}

fn signed_in(answer: &Answer) -> bool { answer.status == 303 && answer.all("set-cookie").iter().any(|c| c.starts_with("__Host-ravenglass-session=")) }

/// Refused with the one generic error for a bad signature, and no session.
fn assert_refused(answer: &Answer, what: &str) {
    assert_eq!(answer.status, 400, "{what}: {}", answer.text());
    assert!(answer.text().contains("signature does not verify"), "{what}: {}", answer.text());
    assert!(!signed_in(answer), "{what}");
}

#[test]
fn rs256_ps256_es256_and_eddsa_tokens_are_accepted() {
    let idp = start_idp();
    let relay = relay_with(&idp, &[]);
    for alg in ["RS256", "PS256", "ES256", "EdDSA"] {
        idp.set(|b| b.alg = alg);
        let answer = sign_in_once(&relay);
        assert!(signed_in(&answer), "{alg}: {} {}", answer.status, answer.text());
    }
    assert_eq!(idp.jwks_fetches(), 1, "the key set is fetched once and cached");
}

#[test]
fn forged_tokens_are_refused_with_the_generic_error() {
    let idp = start_idp();
    let relay = relay_with(&idp, &[]);
    for (forgery, alg) in [
        (Forgery::TamperPayload, "RS256"),
        (Forgery::TamperPayload, "EdDSA"),
        (Forgery::TamperSignature, "PS256"),
        (Forgery::TamperSignature, "ES256"),
        (Forgery::AlgNone, "RS256"),
        (Forgery::Hs256WithPublicKey, "RS256"),
        (Forgery::AlgMismatch, "RS256"),
        (Forgery::CarriedKey, "EdDSA"),
        (Forgery::Unsigned, "RS256"),
    ] {
        idp.set(|b| { b.forgery = forgery; b.alg = alg; });
        assert_refused(&sign_in_once(&relay), &format!("{forgery:?} {alg}"));
    }
    assert_eq!(idp.evil_fetches.load(Ordering::SeqCst), 0, "a token's jku is never fetched");
    assert_eq!(idp.jwks_fetches(), 1, "none of these named an unknown key");
    assert!(relay.store.users().unwrap().iter().all(|u| u.name != "carol@example.org"));
    idp.set(|b| b.forgery = Forgery::None);
    assert!(signed_in(&sign_in_once(&relay)), "a good token still works afterwards");
}

#[test]
fn an_unknown_kid_refetches_the_key_set_once_then_is_refused() {
    let idp = start_idp();
    let relay = relay_with(&idp, &[]);
    assert!(signed_in(&sign_in_once(&relay)));
    assert_eq!(idp.jwks_fetches(), 1);
    advance(&relay, &idp, 61);
    idp.set(|b| b.forgery = Forgery::UnknownKid);
    assert_refused(&sign_in_once(&relay), "unknown kid");
    assert_eq!(idp.jwks_fetches(), 2, "one refetch for the unknown kid");
    assert_refused(&sign_in_once(&relay), "unknown kid again");
    assert_refused(&sign_in_once(&relay), "and again");
    assert_eq!(idp.jwks_fetches(), 2, "no more refetches within a minute");
}

#[test]
fn a_rotated_key_is_picked_up_by_the_refetch() {
    let idp = start_idp();
    let relay = relay_with(&idp, &[]);
    assert!(signed_in(&sign_in_once(&relay)));
    idp.set(|b| { b.rotated = true; b.forgery = Forgery::Rotated; });
    assert_refused(&sign_in_once(&relay), "the refetch is held back right after the first fetch");
    assert_eq!(idp.jwks_fetches(), 1);
    advance(&relay, &idp, 61);
    assert!(signed_in(&sign_in_once(&relay)), "the new key is found");
    assert_eq!(idp.jwks_fetches(), 2);
}

#[test]
fn an_expired_key_set_is_fetched_again() {
    let idp = start_idp();
    let relay = relay_with(&idp, &[]);
    idp.set(|b| b.jwks_max_age = 1);
    assert!(signed_in(&sign_in_once(&relay)));
    advance(&relay, &idp, 299);
    assert!(signed_in(&sign_in_once(&relay)));
    assert_eq!(idp.jwks_fetches(), 1, "max-age=1 is held to five minutes");
    advance(&relay, &idp, 2);
    assert!(signed_in(&sign_in_once(&relay)));
    assert_eq!(idp.jwks_fetches(), 2, "refetched once expired");
    idp.set(|b| b.jwks_max_age = 600);
    advance(&relay, &idp, 301);
    assert!(signed_in(&sign_in_once(&relay)));
    assert_eq!(idp.jwks_fetches(), 3);
    advance(&relay, &idp, 599);
    assert!(signed_in(&sign_in_once(&relay)));
    assert_eq!(idp.jwks_fetches(), 3, "max-age=600 is honoured");
}

#[test]
fn without_a_jwks_uri_signed_tokens_cannot_be_checked_and_are_refused() {
    let idp = start_idp();
    idp.set(|b| b.jwks_uri = false);
    let relay = relay_with(&idp, &[]);
    assert_refused(&sign_in_once(&relay), "no jwks_uri");
    assert_eq!(idp.jwks_fetches(), 0);
}

#[test]
fn with_signature_checks_off_the_tls_back_channel_is_trusted_as_before() {
    let idp = start_idp();
    idp.set(|b| { b.jwks_uri = false; b.forgery = Forgery::Unsigned; });
    let relay = relay_checking(&idp, &["example.org"], false);
    assert!(signed_in(&sign_in_once(&relay)), "an unsigned token from the token endpoint is accepted");
    idp.set(|b| b.wrong_nonce = true);
    let answer = sign_in_once(&relay);
    assert_eq!(answer.status, 400, "the claims are still checked");
    assert!(answer.text().contains("not for this sign-in"), "{}", answer.text());
    assert_eq!(idp.jwks_fetches(), 0, "no keys are fetched");
}

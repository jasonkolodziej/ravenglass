//! OIDC sign-in (authorization code + PKCE + state + nonce) as a state machine.
//!
//! [`start`] makes a flow and the URL to send the browser to. The server then feeds the flow
//! [`Event`]s (the browser's callback, the token endpoint's answer, the userinfo answer) and
//! carries out the [`Effect`] each step returns. No I/O happens here.
//!
//! The ID token's signature is checked against the provider's published keys before any of its
//! claims are read: the flow asks for that with [`Effect::CheckSignature`] and the server answers
//! with [`Event::Signature`] (see [`crate::jose`]). With signature checks turned off, the server
//! answers "valid" at once and, as OIDC Core 3.1.3.7 allows, the TLS connection the Relay opened
//! to the token endpoint stands in for the signature. Either way its `iss`, `aud`, `azp`, `exp`
//! and `nonce` are all checked, and the user is then identified at the userinfo endpoint, whose
//! `sub` must match the ID token's.

use crate::crypto::{random_token, same};
use base64::Engine;
use serde::Deserialize;
use url::Url;

/// How long a sign-in may take between leaving for the identity provider and coming back.
pub const FLOW_LIFETIME: u64 = 10 * 60;
/// Clock skew tolerated on `exp`.
const LEEWAY: u64 = 60;
/// The one error for every ID token whose signature is refused, whatever the reason.
pub const BAD_SIGNATURE: OidcError = OidcError::Token("its signature does not verify");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OidcError {
    Discovery(&'static str),
    /// The callback's `state` or browser binding does not match a flow in progress.
    WrongState,
    /// The flow took longer than [`FLOW_LIFETIME`].
    Expired,
    /// The identity provider reported an error (its `error` code, e.g. `access_denied`).
    Provider(String),
    Token(&'static str),
    WrongNonce,
    UserInfo(&'static str),
    /// Signed in at the identity provider, but not allowed to use this Relay.
    NotAllowed,
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OidcError::Discovery(m) => write!(f, "identity provider discovery failed: {m}"),
            OidcError::WrongState => write!(f, "this sign-in was not started here, or was already used"),
            OidcError::Expired => write!(f, "this sign-in took too long; start again"),
            OidcError::Provider(code) => write!(f, "the identity provider refused: {code}"),
            OidcError::Token(m) => write!(f, "the identity provider's token is not acceptable: {m}"),
            OidcError::WrongNonce => write!(f, "the identity provider's token is not for this sign-in"),
            OidcError::UserInfo(m) => write!(f, "the identity provider's user information is not acceptable: {m}"),
            OidcError::NotAllowed => write!(f, "this account is not allowed to use this relay"),
        }
    }
}

/// The endpoints from `/.well-known/openid-configuration`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Provider {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
    /// Where the provider publishes its signing keys. Required when signatures are checked.
    #[serde(default)]
    pub jwks_uri: Option<String>,
}

/// Where the discovery document for `issuer` lives.
pub fn discovery_url(issuer: &str) -> String { format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/')) }

/// Parse a discovery document. Its `issuer` must be exactly the configured one (OIDC Discovery 4.3).
pub fn parse_discovery(json: &str, issuer: &str) -> Result<Provider, OidcError> {
    let provider: Provider = serde_json::from_str(json).map_err(|_| OidcError::Discovery("not a discovery document"))?;
    if provider.issuer != issuer { return Err(OidcError::Discovery("the issuer does not match the configuration")); }
    for endpoint in [&provider.authorization_endpoint, &provider.token_endpoint, &provider.userinfo_endpoint].into_iter().chain(provider.jwks_uri.as_ref()) {
        if !secure_endpoint(endpoint) { return Err(OidcError::Discovery("every endpoint must be https")); }
    }
    Ok(provider)
}

/// `https://…`, or `http://` to this machine (a development identity provider).
pub fn secure_endpoint(url: &str) -> bool {
    let Ok(url) = Url::parse(url) else { return false };
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => match url.host() {
            Some(url::Host::Domain(d)) => d == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// The Relay's registration at the identity provider, and who may sign in.
#[derive(Clone, Debug)]
pub struct Client {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub scopes: String,
    /// Exact email addresses allowed (lowercase). Both lists empty: anyone the provider vouches for.
    pub allowed_emails: Vec<String>,
    /// Email domains allowed (lowercase, no `@`).
    pub allowed_domains: Vec<String>,
}

impl Client {
    fn restricted(&self) -> bool { !(self.allowed_emails.is_empty() && self.allowed_domains.is_empty()) }

    pub fn permits(&self, email: Option<&str>, verified: bool) -> bool {
        if !self.restricted() { return true; }
        let Some(email) = email.filter(|_| verified).map(str::to_ascii_lowercase) else { return false };
        let domain = email.rsplit_once('@').map(|(_, d)| d).unwrap_or_default();
        self.allowed_emails.contains(&email) || self.allowed_domains.iter().any(|d| d == domain)
    }
}

/// Who signed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// `issuer|sub`: stable and unique across providers.
    pub subject: String,
    pub email: Option<String>,
    /// What to call them: the email, else the preferred username, else the subject.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Sent to the identity provider; waiting for the browser to come back.
    AwaitingCallback { state: String, nonce: String, verifier: String, binding: String, started: u64 },
    /// Traded the code; waiting for the token endpoint.
    AwaitingTokens { nonce: String, started: u64 },
    /// Got the tokens; waiting for the ID token's signature to be checked. `now` is when they came.
    AwaitingSignature { nonce: String, id_token: String, access_token: String, now: u64 },
    /// Checked the ID token; waiting for the userinfo endpoint.
    AwaitingUserInfo { sub: String, token_email: Option<String>, token_verified: bool },
    SignedIn(Identity),
    Failed(OidcError),
}

#[derive(Clone, Debug)]
pub enum Event {
    /// The browser's callback query string, and the flow cookie it carried.
    Callback { query: String, binding: String, now: u64 },
    Tokens { status: u16, body: String, now: u64 },
    /// The outcome of [`Effect::CheckSignature`].
    Signature { valid: bool },
    UserInfo { status: u16, body: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// POST `body` (form-encoded) to the token endpoint, with HTTP Basic client authentication when `basic` is set.
    PostToken { url: String, body: String, basic: Option<String> },
    /// Check the ID token's signature against the provider's keys, then send [`Event::Signature`].
    CheckSignature { id_token: String },
    /// GET the userinfo endpoint with this bearer token.
    GetUserInfo { url: String, access_token: String },
    SignIn(Identity),
    Fail(OidcError),
}

/// A new flow and the authorization URL. `binding` goes into a cookie on the browser that
/// started the flow, so a callback from any other browser (login CSRF) is refused.
pub fn start(provider: &Provider, client: &Client, now: u64) -> (State, String) {
    let (state, nonce, verifier, binding) = (random_token(32), random_token(32), random_token(32), random_token(32));
    let mut url = Url::parse(&provider.authorization_endpoint).expect("checked at discovery");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &client.client_id)
        .append_pair("redirect_uri", &client.redirect_uri)
        .append_pair("scope", &client.scopes)
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge(&verifier))
        .append_pair("code_challenge_method", "S256");
    (State::AwaitingCallback { state, nonce, verifier, binding, started: now }, url.into())
}

fn challenge(verifier: &str) -> String {
    use sha2::Digest;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn fail(error: OidcError) -> (State, Option<Effect>) { (State::Failed(error.clone()), Some(Effect::Fail(error))) }

fn form(pairs: &[(&str, &str)]) -> String { url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish() }

fn basic(client: &Client) -> Option<String> {
    let secret = client.client_secret.as_deref()?;
    // RFC 6749 §2.3.1: both parts form-encoded before Base64.
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    Some(base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", enc(&client.client_id), enc(secret))))
}

impl State {
    pub fn step(self, provider: &Provider, client: &Client, event: Event) -> (State, Option<Effect>) {
        match (self, event) {
            (State::AwaitingCallback { state, nonce, verifier, binding, started }, Event::Callback { query, binding: presented, now }) => {
                let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
                let get = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
                if !same(get("state").unwrap_or_default().as_bytes(), state.as_bytes()) || !same(presented.as_bytes(), binding.as_bytes()) { return fail(OidcError::WrongState); }
                if now >= started + FLOW_LIFETIME { return fail(OidcError::Expired); }
                if let Some(error) = get("error") { return fail(OidcError::Provider(error.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(40).collect())); }
                let Some(code) = get("code").filter(|c| !c.is_empty()) else { return fail(OidcError::Provider("no_code".into())) };
                let mut fields = vec![("grant_type", "authorization_code"), ("code", code), ("redirect_uri", client.redirect_uri.as_str()), ("code_verifier", verifier.as_str())];
                if client.client_secret.is_none() { fields.push(("client_id", client.client_id.as_str())); }
                let effect = Effect::PostToken { url: provider.token_endpoint.clone(), body: form(&fields), basic: basic(client) };
                (State::AwaitingTokens { nonce, started }, Some(effect))
            }
            (State::AwaitingTokens { nonce, .. }, Event::Tokens { status, body, now }) => {
                if status != 200 { return fail(OidcError::Token("the token endpoint refused the code")); }
                let Ok(tokens) = serde_json::from_str::<TokenResponse>(&body) else { return fail(OidcError::Token("not a token response")) };
                if !tokens.token_type.eq_ignore_ascii_case("bearer") { return fail(OidcError::Token("not a bearer token")); }
                let effect = Effect::CheckSignature { id_token: tokens.id_token.clone() };
                (State::AwaitingSignature { nonce, id_token: tokens.id_token, access_token: tokens.access_token, now }, Some(effect))
            }
            (State::AwaitingSignature { nonce, id_token, access_token, now }, Event::Signature { valid }) => {
                if !valid { return fail(BAD_SIGNATURE); }
                let claims = match id_token_claims(&id_token) { Ok(c) => c, Err(e) => return fail(e) };
                if claims.iss != provider.issuer { return fail(OidcError::Token("wrong issuer")); }
                let audiences = claims.aud.list();
                if !audiences.iter().any(|a| *a == client.client_id) { return fail(OidcError::Token("not issued to this relay")); }
                if (audiences.len() > 1 || claims.azp.is_some()) && claims.azp.as_deref() != Some(client.client_id.as_str()) { return fail(OidcError::Token("wrong authorized party")); }
                if claims.exp + LEEWAY <= now { return fail(OidcError::Token("expired")); }
                if !same(claims.nonce.as_deref().unwrap_or_default().as_bytes(), nonce.as_bytes()) { return fail(OidcError::WrongNonce); }
                if claims.sub.is_empty() { return fail(OidcError::Token("no subject")); }
                let effect = Effect::GetUserInfo { url: provider.userinfo_endpoint.clone(), access_token };
                (State::AwaitingUserInfo { sub: claims.sub, token_email: claims.email, token_verified: claims.email_verified.unwrap_or(false) }, Some(effect))
            }
            (State::AwaitingUserInfo { sub, token_email, token_verified }, Event::UserInfo { status, body }) => {
                if status != 200 { return fail(OidcError::UserInfo("the userinfo endpoint refused")); }
                let Ok(info) = serde_json::from_str::<UserInfo>(&body) else { return fail(OidcError::UserInfo("not a userinfo response")) };
                if info.sub != sub { return fail(OidcError::UserInfo("a different subject than the ID token")); }
                let (email, verified) = match info.email { Some(e) => (Some(e), info.email_verified.unwrap_or(false)), None => (token_email, token_verified) };
                if !client.permits(email.as_deref(), verified) { return fail(OidcError::NotAllowed); }
                let name = email.clone().or(info.preferred_username).unwrap_or_else(|| sub.clone());
                let identity = Identity { subject: format!("{}|{sub}", provider.issuer), email, name };
                (State::SignedIn(identity.clone()), Some(Effect::SignIn(identity)))
            }
            (State::Failed(e), _) => (State::Failed(e), None),
            (_, _) => fail(OidcError::WrongState),
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse { access_token: String, id_token: String, token_type: String }

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience { One(String), Many(Vec<String>) }

impl Audience {
    fn list(&self) -> Vec<&str> { match self { Audience::One(a) => vec![a.as_str()], Audience::Many(a) => a.iter().map(String::as_str).collect() } }
}

#[derive(Deserialize)]
struct Claims { iss: String, sub: String, aud: Audience, exp: u64, nonce: Option<String>, azp: Option<String>, email: Option<String>, email_verified: Option<bool> }

#[derive(Deserialize)]
struct UserInfo { sub: String, email: Option<String>, email_verified: Option<bool>, preferred_username: Option<String> }

fn id_token_claims(token: &str) -> Result<Claims, OidcError> {
    let mut parts = token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else { return Err(OidcError::Token("not a JWT")) };
    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).map_err(|_| OidcError::Token("not a JWT"))?;
    serde_json::from_slice(&json).map_err(|_| OidcError::Token("missing claims"))
}

#[cfg(test)]
mod tests;

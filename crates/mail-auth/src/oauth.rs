//! OAuth2 authorization-code flow with PKCE (RFC 7636) for a desktop app: the browser signs the
//! user in and redirects to a loopback port. This module builds the URLs and request bodies and
//! reads the answers; the caller opens the browser and performs the HTTPS POST.
//!
//! A provider needs a client id registered by whoever ships the app. None is built in.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider { Google, Microsoft }

pub struct Endpoints { pub authorize: &'static str, pub token: &'static str, pub scopes: &'static str }

impl Provider {
    pub fn endpoints(self) -> Endpoints {
        match self {
            Provider::Google => Endpoints { authorize: "https://accounts.google.com/o/oauth2/v2/auth", token: "https://oauth2.googleapis.com/token", scopes: "https://mail.google.com/" },
            Provider::Microsoft => Endpoints {
                authorize: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
                token: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                scopes: "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access",
            },
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OAuthError {
    #[error("no secure random numbers: {0}")]
    Random(String),
    #[error("the sign-in was cancelled or refused: {0}")]
    Denied(String),
    #[error("the redirect did not come from this sign-in")]
    StateMismatch,
    #[error("unexpected answer from the provider: {0}")]
    BadResponse(String),
    /// The refresh token no longer works; the user has to sign in again.
    #[error("sign in again")]
    Expired,
    #[error("local redirect listener: {0}")]
    Listener(String),
}

/// One sign-in attempt: the PKCE verifier and the anti-forgery state. Keep it until the redirect.
pub struct Attempt { verifier: String, pub state: String }

impl Attempt {
    pub fn new() -> Result<Self, OAuthError> {
        let random = || -> Result<String, OAuthError> {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).map_err(|e| OAuthError::Random(e.to_string()))?;
            Ok(URL_SAFE_NO_PAD.encode(bytes))
        };
        Ok(Self { verifier: random()?, state: random()? })
    }

    fn challenge(&self) -> String { URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes())) }

    /// The URL to open in the user's browser.
    pub fn authorize_url(&self, provider: Provider, client_id: &str, redirect_uri: &str, login_hint: &str) -> String {
        let e = provider.endpoints();
        let mut url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256&login_hint={}",
            e.authorize, form_encode(client_id), form_encode(redirect_uri), form_encode(e.scopes), self.state, self.challenge(), form_encode(login_hint),
        );
        // Google only returns a refresh token when asked for offline access with consent.
        if provider == Provider::Google { url.push_str("&access_type=offline&prompt=consent"); }
        url
    }

    /// The authorization code from the redirect's query string, after checking the state.
    pub fn code(&self, query: &str) -> Result<String, OAuthError> {
        let get = |key: &str| query.trim_start_matches('?').split('&').filter_map(|p| p.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| form_decode(v));
        if let Some(error) = get("error") { return Err(OAuthError::Denied(error)); }
        if get("state").as_deref() != Some(self.state.as_str()) { return Err(OAuthError::StateMismatch); }
        get("code").filter(|c| !c.is_empty()).ok_or_else(|| OAuthError::BadResponse("no code".into()))
    }

    /// The form body to POST to the token endpoint to trade the code for tokens.
    pub fn token_request(&self, client_id: &str, redirect_uri: &str, code: &str) -> String {
        format!("grant_type=authorization_code&client_id={}&redirect_uri={}&code={}&code_verifier={}", form_encode(client_id), form_encode(redirect_uri), form_encode(code), self.verifier)
    }
}

/// The form body to POST to the token endpoint for a new access token.
pub fn refresh_request(client_id: &str, refresh_token: &str) -> String {
    format!("grant_type=refresh_token&client_id={}&refresh_token={}", form_encode(client_id), form_encode(refresh_token))
}

/// Tokens in memory. The refresh token goes to the keyring; the access token is never stored.
#[derive(Clone, PartialEq)]
pub struct Tokens { pub access: String, pub refresh: Option<String>, pub expires_at: f64 }

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens").field("access", &"…").field("refresh", &self.refresh.as_ref().map(|_| "…")).field("expires_at", &self.expires_at).finish()
    }
}

impl Tokens {
    /// Refresh a minute early so a request never goes out with a token about to lapse.
    pub fn needs_refresh(&self, now: f64) -> bool { now >= self.expires_at - 60.0 }
}

/// Read the token endpoint's JSON answer. `now` is when it arrived.
pub fn parse_tokens(json: &str, now: f64) -> Result<Tokens, OAuthError> {
    #[derive(Deserialize)]
    struct Answer { access_token: Option<String>, refresh_token: Option<String>, expires_in: Option<f64>, error: Option<String> }
    let answer: Answer = serde_json::from_str(json).map_err(|e| OAuthError::BadResponse(e.to_string()))?;
    match (answer.access_token, answer.error.as_deref()) {
        (Some(access), _) if !access.is_empty() => Ok(Tokens { access, refresh: answer.refresh_token, expires_at: now + answer.expires_in.unwrap_or(3600.0) }),
        (_, Some("invalid_grant")) => Err(OAuthError::Expired),
        (_, Some(error)) => Err(OAuthError::Denied(error.to_owned())),
        _ => Err(OAuthError::BadResponse("no access token".into())),
    }
}

/// `application/x-www-form-urlencoded` value encoding.
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') { out.push(b as char); } else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

fn form_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        match b {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = [bytes.next().unwrap_or(b'0'), bytes.next().unwrap_or(b'0')];
                out.push(std::str::from_utf8(&hex).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(b'?'));
            }
            _ => out.push(b),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The loopback redirect (RFC 8252 §7.3): listen on a free port of this machine only.
#[cfg(not(target_arch = "wasm32"))]
pub mod loopback {
    use super::OAuthError;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    pub struct Redirect { listener: TcpListener }

    impl Redirect {
        pub fn listen() -> Result<Self, OAuthError> {
            TcpListener::bind(("127.0.0.1", 0)).map(|listener| Self { listener }).map_err(|e| OAuthError::Listener(e.to_string()))
        }

        /// The `redirect_uri` to register with this attempt.
        pub fn uri(&self) -> Result<String, OAuthError> {
            self.listener.local_addr().map(|a| format!("http://127.0.0.1:{}/", a.port())).map_err(|e| OAuthError::Listener(e.to_string()))
        }

        /// Block until the browser arrives, answer it, and return the query string.
        pub fn wait(self) -> Result<String, OAuthError> {
            let fail = |e: std::io::Error| OAuthError::Listener(e.to_string());
            loop {
                let (mut stream, _) = self.listener.accept().map_err(fail)?;
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).map_err(fail)?;
                // "GET /?code=…&state=… HTTP/1.1". Anything else (a favicon request) is not it.
                let query = line.split_whitespace().nth(1).and_then(|path| path.strip_prefix("/?"));
                let body = if query.is_some() { "Signed in. You can close this tab and return to Ravenglass." } else { "" };
                let status = if query.is_some() { "200 OK" } else { "404 Not Found" };
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                if let Some(query) = query { return Ok(query.to_owned()); }
            }
        }
    }
}

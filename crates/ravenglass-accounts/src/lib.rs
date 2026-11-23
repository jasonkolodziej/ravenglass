//! From settings to a working account (architecture §4): pick the backend an [`AccountConfig`]
//! names, fetch its secret from the keyring, trade a refresh token for an access token when the
//! account uses OAuth, and hand back a [`MailBackend`] for the engine.
//!
//! Nothing connects here: backends connect on first use, so a wrong password shows up as the
//! sync machine's `AuthFailed`, not as an error from [`Connector::connect`].
//!
//! OAuth needs a client id registered with the provider by whoever ships the app. None is built
//! in; without one, Gmail and Microsoft accounts report [`ConnectError::NoClientId`].

#[cfg(feature = "http")]
pub mod http;
#[cfg(test)]
mod tests;

use mail_auth::oauth::{self, OAuthError, Provider, Tokens};
use mail_auth::secrets::{SecretError, SecretKind, SecretStore};
use mail_auth::{AccountConfig, AuthKind, Incoming, Security, Server};
use mail_core::{Address, MailBackend};
use mail_imap::{Credential, Endpoint, ImapBackend, ImapConfig};
use std::sync::Arc;

/// POST a form body to a URL and return the response body. Errors are text for the status bar.
pub type FormPost = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("no password is stored for this account")]
    NoPassword,
    /// The user has to go through the browser sign-in (again).
    #[error("sign in to this account")]
    SignIn,
    #[error("this build has no {0:?} OAuth client id")]
    NoClientId(Provider),
    #[error("JMAP accounts need the `http` feature")]
    NoJmapTransport,
    #[error(transparent)]
    OAuth(#[from] OAuthError),
    #[error(transparent)]
    Secrets(#[from] SecretError),
    #[error("could not reach the sign-in service: {0}")]
    Network(String),
    #[error(transparent)]
    Backend(#[from] mail_core::MailError),
}

/// OAuth client ids, per provider.
#[derive(Clone, Debug, Default)]
pub struct ClientIds { pub google: Option<String>, pub microsoft: Option<String> }

impl ClientIds {
    fn get(&self, provider: Provider) -> Result<&str, ConnectError> {
        match provider { Provider::Google => &self.google, Provider::Microsoft => &self.microsoft }.as_deref().ok_or(ConnectError::NoClientId(provider))
    }
}

pub struct Connector {
    pub secrets: Arc<dyn SecretStore>,
    pub client_ids: ClientIds,
    /// How token requests are sent. `None`: OAuth accounts cannot connect.
    pub post: Option<FormPost>,
}

/// A configured account, ready for `Engine::add_backend` and `Engine::set_identity`.
pub struct Connected {
    pub backend: Arc<dyn MailBackend>,
    pub identity: Address,
    /// When the OAuth access token lapses; call [`Connector::refresh`] before then.
    pub token_expires_at: Option<f64>,
    imap: Option<Arc<ImapBackend>>,
}

impl Connector {
    /// Build the account's backend. `now` is seconds since the Unix epoch.
    pub fn connect(&self, config: &AccountConfig, now: f64) -> Result<Connected, ConnectError> {
        if let Some(problem) = config.problem() { return Err(ConnectError::Invalid(problem)); }
        let (secret, token_expires_at) = self.secret(config, now)?;
        let identity = Address { name: (!config.name.is_empty()).then(|| config.name.clone()), email: config.address.clone() };
        match &config.incoming {
            Incoming::Imap(imap) => {
                let smtp = config.smtp.as_ref().ok_or(ConnectError::Invalid("an IMAP account needs an SMTP server"))?;
                let credential = match &secret { Secret::Password(p) => Credential::Password(p.clone()), Secret::Token(t) => Credential::OAuth2 { access_token: t.clone() } };
                let mut settings = ImapConfig::new(&config.id.0, &imap.host, &smtp.host, &config.username, credential);
                settings.imap = endpoint(imap);
                settings.smtp = endpoint(smtp);
                settings.save_sent = !config.server_saves_sent;
                let backend = Arc::new(ImapBackend::new(settings)?);
                Ok(Connected { backend: backend.clone(), identity, token_expires_at, imap: Some(backend) })
            }
            Incoming::Jmap { session_url } => Ok(Connected { backend: jmap(config, session_url, &secret)?, identity, token_expires_at, imap: None }),
        }
    }

    /// Give a connected OAuth account a fresh access token. A no-op for password accounts.
    pub fn refresh(&self, connected: &mut Connected, config: &AccountConfig, now: f64) -> Result<(), ConnectError> {
        if !matches!(config.auth, AuthKind::OAuth2(_)) { return Ok(()); }
        let (secret, expires) = self.secret(config, now)?;
        match (&connected.imap, secret) {
            (Some(imap), Secret::Token(access_token)) => imap.set_credential(Credential::OAuth2 { access_token }),
            // A JMAP transport holds its credential for life, so the backend is rebuilt.
            _ => *connected = self.connect(config, now)?,
        }
        connected.token_expires_at = expires;
        Ok(())
    }

    /// Finish a browser sign-in: trade the authorization code for tokens and keep the refresh token.
    pub fn complete_sign_in(&self, config: &AccountConfig, attempt: &oauth::Attempt, redirect_uri: &str, query: &str, now: f64) -> Result<(), ConnectError> {
        let AuthKind::OAuth2(provider) = config.auth else { return Err(ConnectError::Invalid("this account does not use OAuth")) };
        let code = attempt.code(query)?;
        let body = attempt.token_request(self.client_ids.get(provider)?, redirect_uri, &code);
        let tokens = self.request(provider, &body, now)?;
        let refresh = tokens.refresh.ok_or(OAuthError::BadResponse("the provider returned no refresh token".into()))?;
        Ok(self.secrets.set(&config.id, SecretKind::RefreshToken, &refresh)?)
    }

    fn request(&self, provider: Provider, body: &str, now: f64) -> Result<Tokens, ConnectError> {
        let post = self.post.as_ref().ok_or_else(|| ConnectError::Network("no HTTP client".into()))?;
        // Providers answer refusals with a JSON error body, so a failed POST may still carry one.
        let answer = post(provider.endpoints().token, body).map_err(ConnectError::Network)?;
        Ok(oauth::parse_tokens(&answer, now)?)
    }

    fn secret(&self, config: &AccountConfig, now: f64) -> Result<(Secret, Option<f64>), ConnectError> {
        match config.auth {
            AuthKind::Password => Ok((Secret::Password(self.secrets.get(&config.id, SecretKind::Password)?.ok_or(ConnectError::NoPassword)?), None)),
            AuthKind::OAuth2(provider) => {
                let client_id = self.client_ids.get(provider)?;
                let refresh = self.secrets.get(&config.id, SecretKind::RefreshToken)?.ok_or(ConnectError::SignIn)?;
                let tokens = match self.request(provider, &oauth::refresh_request(client_id, &refresh), now) {
                    Err(ConnectError::OAuth(OAuthError::Expired)) => {
                        self.secrets.delete(&config.id, SecretKind::RefreshToken)?;
                        return Err(ConnectError::SignIn);
                    }
                    other => other?,
                };
                // Some providers rotate the refresh token on every use.
                if let Some(rotated) = tokens.refresh.as_deref().filter(|r| *r != refresh) { self.secrets.set(&config.id, SecretKind::RefreshToken, rotated)?; }
                Ok((Secret::Token(tokens.access), Some(tokens.expires_at)))
            }
        }
    }
}

enum Secret { Password(String), Token(String) }

fn endpoint(server: &Server) -> Endpoint {
    let security = match server.security { Security::Tls => mail_imap::Security::Tls, Security::StartTls => mail_imap::Security::StartTls, Security::Plain => mail_imap::Security::Plain };
    Endpoint::new(&server.host, server.port, security)
}

#[cfg(feature = "http")]
fn jmap(config: &AccountConfig, session_url: &str, secret: &Secret) -> Result<Arc<dyn MailBackend>, ConnectError> {
    use mail_jmap::{Credential as Jmap, HttpsTransport, JmapBackend};
    let credential = match secret { Secret::Password(p) => Jmap::Basic { username: config.username.clone(), password: p.clone() }, Secret::Token(t) => Jmap::Bearer(t.clone()) };
    Ok(Arc::new(JmapBackend::new(config.id.clone(), session_url, HttpsTransport::new(&credential))))
}

#[cfg(not(feature = "http"))]
fn jmap(_: &AccountConfig, _: &str, _: &Secret) -> Result<Arc<dyn MailBackend>, ConnectError> { Err(ConnectError::NoJmapTransport) }

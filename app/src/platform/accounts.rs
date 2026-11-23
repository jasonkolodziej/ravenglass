//! Account connections, per target. Desktop builds connect to mail servers themselves through
//! `ravenglass-accounts` (IMAP, SMTP, JMAP, OAuth over a loopback redirect). A browser cannot
//! open those sockets, so the web build reaches mail only through the Relay (`crate::web`), and
//! here it gets the same names with nothing behind them: every profile account reports that it
//! needs the Relay, and the shell code that uses them compiles unchanged.

#[cfg(not(target_arch = "wasm32"))]
pub use mail_auth::oauth::loopback::Redirect;
#[cfg(not(target_arch = "wasm32"))]
pub use ravenglass_accounts::{ClientIds, ConnectError, Connected, Connector, FormPost, http};

#[cfg(target_arch = "wasm32")]
pub use web::*;

#[cfg(target_arch = "wasm32")]
mod web {
    use mail_auth::oauth::{Attempt, OAuthError, Provider};
    use mail_auth::{AccountConfig, SecretStore};
    use mail_core::{Address, MailBackend};
    use std::sync::Arc;

    const RELAY_ONLY: &str = "the web app reaches mail through the relay only";

    pub type FormPost = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

    #[derive(Debug)]
    #[allow(dead_code)] // the variants the shell names; only `Invalid` is made on the web
    pub enum ConnectError {
        Invalid(&'static str),
        NoPassword,
        SignIn,
        NoClientId(Provider),
        Network(String),
    }

    impl std::fmt::Display for ConnectError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                ConnectError::Invalid(why) => f.write_str(why),
                ConnectError::NoPassword => f.write_str("no password is stored for this account"),
                ConnectError::SignIn => f.write_str("sign in to this account"),
                ConnectError::NoClientId(p) => write!(f, "this build has no {p:?} OAuth client id"),
                ConnectError::Network(why) => write!(f, "could not reach the sign-in service: {why}"),
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct ClientIds { pub google: Option<String>, pub microsoft: Option<String> }

    pub struct Connector {
        pub secrets: Arc<dyn SecretStore>,
        pub client_ids: ClientIds,
        pub post: Option<FormPost>,
    }

    pub struct Connected {
        pub backend: Arc<dyn MailBackend>,
        pub identity: Address,
        pub token_expires_at: Option<f64>,
    }

    impl Connector {
        pub fn connect(&self, _config: &AccountConfig, _now: f64) -> Result<Connected, ConnectError> { Err(ConnectError::Invalid(RELAY_ONLY)) }
        pub fn refresh(&self, _connected: &mut Connected, _config: &AccountConfig, _now: f64) -> Result<(), ConnectError> { Err(ConnectError::Invalid(RELAY_ONLY)) }
        pub fn complete_sign_in(&self, _config: &AccountConfig, _attempt: &Attempt, _redirect_uri: &str, _query: &str, _now: f64) -> Result<(), ConnectError> {
            Err(ConnectError::Invalid(RELAY_ONLY))
        }
    }

    /// No HTTPS of the app's own on the web: autoconfig and OAuth token requests need a server.
    pub mod http {
        pub fn fetch_document(_url: &str) -> Option<String> { None }
        pub fn form_post() -> super::FormPost { std::sync::Arc::new(|_, _| Err(super::RELAY_ONLY.into())) }
    }

    /// The loopback OAuth redirect cannot exist in a browser: nothing can listen on a port.
    pub struct Redirect;

    impl Redirect {
        pub fn listen() -> Result<Redirect, OAuthError> { Err(OAuthError::Listener(RELAY_ONLY.into())) }
        pub fn uri(&self) -> Result<String, OAuthError> { Err(OAuthError::Listener(RELAY_ONLY.into())) }
        pub fn wait(self) -> Result<String, OAuthError> { Err(OAuthError::Listener(RELAY_ONLY.into())) }
    }
}

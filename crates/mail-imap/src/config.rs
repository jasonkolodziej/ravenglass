//! Account settings for one IMAP + SMTP login. Plain data; nothing here touches the network.

use mail_core::AccountId;
use std::fmt;
use std::time::Duration;

/// How a connection is protected. There is no "opportunistic" mode: `StartTls` fails if the
/// server will not upgrade, so a connection is never silently downgraded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte (IMAP 993, SMTP 465).
    Tls,
    /// Plain connect, then a mandatory STARTTLS upgrade before any credential is sent (IMAP 143, SMTP 587).
    StartTls,
    /// No encryption. For local test servers only.
    Plain,
}

/// What proves the login. Getting and refreshing OAuth tokens is `mail-auth`'s job; hand a fresh
/// one over with [`crate::ImapBackend::set_credential`].
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    /// An account password or an app-specific password (IMAP `LOGIN`, SMTP `AUTH PLAIN`/`LOGIN`).
    Password(String),
    /// An OAuth 2.0 access token (SASL `XOAUTH2` on both protocols).
    OAuth2 { access_token: String },
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self { Credential::Password(_) => "Password(…)", Credential::OAuth2 { .. } => "OAuth2 { … }" })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint { pub host: String, pub port: u16, pub security: Security }

impl Endpoint {
    pub fn new(host: &str, port: u16, security: Security) -> Self { Self { host: host.into(), port, security } }
}

#[derive(Clone)]
pub struct ImapConfig {
    /// Names the account in the engine and prefixes every folder and message id.
    pub account: AccountId,
    pub imap: Endpoint,
    pub smtp: Endpoint,
    pub username: String,
    pub credential: Credential,
    /// APPEND a copy to the Sent folder after SMTP accepts a message. Turn it off where the
    /// server files sent mail itself (Gmail), or every message is saved twice.
    pub save_sent: bool,
    /// Extra trust anchors (DER certificates) next to the bundled webpki roots: a private CA for a
    /// self-hosted server, or a test server's own certificate.
    pub extra_roots: Vec<Vec<u8>>,
    /// Capabilities to treat as absent even when the server advertises them (`"MOVE"`, `"IDLE"`,
    /// `"UIDPLUS"`, `"CONDSTORE"`). A workaround for servers with a broken extension, and how the
    /// tests reach the fallback paths.
    pub disabled_capabilities: Vec<String>,
    /// Longest wait for a connect, a TLS handshake or the next piece of any server response.
    pub timeout: Duration,
    /// How often `idle` asks a server that has no IDLE whether anything changed.
    pub idle_poll: Duration,
}

impl ImapConfig {
    /// TLS on the standard ports (IMAP 993, SMTP 465) with a password.
    pub fn new(account: &str, imap_host: &str, smtp_host: &str, username: &str, credential: Credential) -> Self {
        Self {
            account: AccountId::from(account),
            imap: Endpoint::new(imap_host, 993, Security::Tls),
            smtp: Endpoint::new(smtp_host, 465, Security::Tls),
            username: username.into(),
            credential,
            save_sent: true,
            extra_roots: Vec::new(),
            disabled_capabilities: Vec::new(),
            timeout: Duration::from_secs(60),
            idle_poll: Duration::from_secs(60),
        }
    }
}

/// Never prints the username (an address) or the credential.
impl fmt::Debug for ImapConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImapConfig").field("account", &self.account).field("imap", &self.imap).field("smtp", &self.smtp)
            .field("credential", &self.credential).field("save_sent", &self.save_sent).finish_non_exhaustive()
    }
}

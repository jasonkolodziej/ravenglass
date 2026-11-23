//! An account's settings. No secrets: those live in the keyring, keyed by the account id.

use crate::oauth::Provider;
use mail_core::AccountId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Security {
    /// TLS from the first byte (IMAP 993, SMTP 465).
    Tls,
    /// Upgrade with STARTTLS; the connection fails if the server does not offer it.
    StartTls,
    /// No encryption. Refused for anything but a loopback test server.
    Plain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server { pub host: String, pub port: u16, pub security: Security }

impl Server {
    pub fn new(host: &str, port: u16, security: Security) -> Self { Self { host: host.to_owned(), port, security } }

    /// Plain connections are allowed only to this machine.
    pub fn is_acceptable(&self) -> bool {
        self.security != Security::Plain || matches!(self.host.as_str(), "localhost" | "127.0.0.1" | "::1")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Incoming {
    Imap(Server),
    /// JMAP session resource URL (RFC 8620 §2).
    Jmap { session_url: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthKind {
    /// A password or an app-specific password.
    Password,
    OAuth2(Provider),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountConfig {
    pub id: AccountId,
    /// The name mail is sent as.
    pub name: String,
    pub address: String,
    /// The login name; usually the address.
    pub username: String,
    pub incoming: Incoming,
    /// SMTP submission. `None` for JMAP accounts, which submit over JMAP.
    pub smtp: Option<Server>,
    pub auth: AuthKind,
    /// The server files sent mail itself (Gmail), so the client must not append it to Sent.
    pub server_saves_sent: bool,
}

impl AccountConfig {
    /// Why this account cannot be used, if it cannot.
    pub fn problem(&self) -> Option<&'static str> {
        if !self.address.contains('@') { return Some("the address is not an email address"); }
        if self.username.is_empty() { return Some("the user name is empty"); }
        let servers = match &self.incoming { Incoming::Imap(s) => vec![Some(s), self.smtp.as_ref()], Incoming::Jmap { .. } => vec![self.smtp.as_ref()] };
        if servers.into_iter().flatten().any(|s| !s.is_acceptable()) { return Some("unencrypted connections are only allowed to this machine"); }
        match &self.incoming {
            Incoming::Imap(_) if self.smtp.is_none() => Some("an IMAP account needs an SMTP server"),
            Incoming::Jmap { session_url } if !session_url.starts_with("https://") => Some("the JMAP URL must be https"),
            _ => None,
        }
    }
}

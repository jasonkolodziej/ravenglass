//! Accounts (architecture §4, §8, §12): what it takes to reach a mailbox, and nothing that
//! touches the network itself.
//!
//! - [`config`]: an account's server settings, serializable and free of secrets.
//! - [`autoconfig`]: settings from an email address: built-in providers, Mozilla ISPDB XML, then
//!   conventional host names. The caller does the HTTP fetch.
//! - [`oauth`]: OAuth2 authorization-code flow with PKCE for Gmail and Microsoft 365: URLs,
//!   request bodies, response parsing, token expiry, and a loopback redirect listener. The
//!   caller does the HTTPS POST.
//! - [`secrets`]: passwords and refresh tokens in the OS keyring. They are never written to
//!   config files, the store or logs, and none of these types print a secret in `Debug`.
//!
//! - [`passcode`]: the app-lock passcode, stored only as an Argon2id hash.
//!
//! Not here yet: SRV-record lookup.

pub mod autoconfig;
pub mod config;
pub mod oauth;
pub mod passcode;
pub mod secrets;

pub use config::{AccountConfig, AuthKind, Incoming, Security, Server};
pub use secrets::{MemorySecrets, SecretKind, SecretStore};

#[cfg(test)]
mod tests;

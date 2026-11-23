//! IMAP + SMTP backend implementing `mail_core::MailBackend` for any standard host
//! (architecture §4, §5): IMAP4rev1 with MOVE, UIDPLUS, CONDSTORE, IDLE and SPECIAL-USE when the
//! server has them and portable fallbacks when it does not; SMTP submission; password or XOAUTH2.
//!
//! ```no_run
//! use mail_imap::{Credential, ImapBackend, ImapConfig};
//! let config = ImapConfig::new("Work", "imap.example.org", "smtp.example.org", "me@example.org", Credential::Password("app-password".into()));
//! let backend = ImapBackend::new(config).unwrap(); // connects on first use
//! ```
//!
//! Layout: [`config`] (settings), [`ids`] (folder ids, message ids, sync tokens), [`folders`]
//! (roles), [`envelope`] (list rows from FETCH data), [`idle`] (push), and privately the
//! connection, the per-call IMAP operations, SMTP and the runtime bridge.
//!
//! Nothing in this crate logs. Errors carry no message content, addresses or credentials.
//!
//! Integration tests run against GreenMail: `tools/greenmail.sh start`, then
//! `RAVENGLASS_IMAP_TEST=1 cargo test -p mail-imap`.

mod backend;
pub mod config;
mod conn;
pub mod envelope;
pub mod folders;
pub mod idle;
pub mod ids;
mod net;
mod ops;
mod smtp;
#[cfg(test)]
mod tests;

pub use backend::ImapBackend;
pub use config::{Credential, Endpoint, ImapConfig, Security};
pub use net::certificates_from_pem;

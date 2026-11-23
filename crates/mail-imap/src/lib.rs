//! IMAP + SMTP backend implementing `mail_core::MailBackend`.
//! Milestone M5/M7 (plan §6). Suggested: `async-imap` + `tokio-rustls`, `lettre` (rustls) for SMTP.
//!
//! TODO(M5): login (password / app password), folder list, CONDSTORE/QRESYNC sync, lazy bodies,
//! IDLE stream, MOVE (else COPY+EXPUNGE). Integration tests against Greenmail (`tools/docker-compose.yml`).
//! TODO(M7): SMTP send, APPEND to Sent unless the server does it (Gmail).

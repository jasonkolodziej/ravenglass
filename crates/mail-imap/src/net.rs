//! Sockets, TLS and error mapping shared by the IMAP and SMTP sides.
//!
//! Error text that leaves this crate never carries what a server echoed about a user: response
//! text is cut short and anything shaped like an address is replaced.

use crate::config::Endpoint;
use mail_core::{MailError, Result};
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// A connected byte stream: a TCP socket or TLS over one.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + fmt::Debug> Io for T {}
pub type BoxIo = Box<dyn Io>;

/// An error plus whether the connection it happened on is still in step with the server. A tagged
/// NO or BAD leaves it usable; a timeout, a dropped socket or an unreadable response does not.
#[derive(Debug)]
pub struct Failure { pub error: MailError, pub reusable: bool }

/// What a response async-imap could not parse is reported as. `sync_folder` reacts to it by
/// asking for less (see `ops::fetch_envelopes`).
pub const UNREADABLE: &str = "unreadable server response";

impl Failure {
    pub fn broken(error: MailError) -> Self { Self { error, reusable: false } }
}

impl From<MailError> for Failure {
    fn from(error: MailError) -> Self { Self { error, reusable: true } }
}

impl From<async_imap::error::Error> for Failure {
    fn from(error: async_imap::error::Error) -> Self {
        use async_imap::error::Error;
        match error {
            // async-imap reports a response it cannot parse as an `Other` I/O error whose text
            // quotes the response, which may be mail. Never pass that text on.
            Error::Io(e) if e.kind() == std::io::ErrorKind::Other => Self::broken(MailError::Server(UNREADABLE.into())),
            Error::Io(e) => Self::broken(io_error(&e)),
            Error::ConnectionLost => Self::broken(MailError::Network("connection lost".into())),
            Error::No(text) | Error::Bad(text) => MailError::Server(server_text(&text)).into(),
            Error::Validate(_) => MailError::Server("a name contains a character IMAP cannot send".into()).into(),
            Error::Append => Self::broken(MailError::Server("the server did not accept the message".into())),
            _ => Self::broken(MailError::Server("unexpected server response".into())),
        }
    }
}

pub fn io_error(error: &std::io::Error) -> MailError { MailError::Network(error.kind().to_string()) }

/// Run one network step with a deadline. Running out of time leaves the connection unusable.
pub async fn timed<T, E: Into<Failure>>(limit: Duration, step: impl Future<Output = std::result::Result<T, E>>) -> std::result::Result<T, Failure> {
    match tokio::time::timeout(limit, step).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => Err(Failure::broken(MailError::Network("timed out".into()))),
    }
}

/// The human-readable part of a server's refusal, safe to show: at most 160 characters, with
/// anything that looks like an address removed. Accepts plain response text and the two shapes
/// async-imap wraps it in (`code: …, info: Some("…")` and `outcome: Outcome { …, information: Some("…") }`).
pub fn server_text(raw: &str) -> String {
    let wrapped = raw.starts_with("code: ") || raw.starts_with("outcome: ");
    let quoted = ["information: Some(\"", "info: Some(\""].iter().find_map(|marker| raw.split_once(marker))
        .map(|(_, rest)| rest.rsplit_once("\")").map_or(rest, |(text, _)| text));
    let text = quoted.unwrap_or(if wrapped { "" } else { raw });
    let cleaned: Vec<&str> = text.split_whitespace().map(|word| if word.contains('@') { "<address>" } else { word }).collect();
    let mut out: String = cleaned.join(" ").chars().take(160).collect();
    if out.is_empty() { out.push_str("request refused"); }
    out
}

/// The bundled webpki roots plus the account's extra trust anchors, on the `ring` provider.
pub fn tls_config(extra_roots: &[Vec<u8>]) -> Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for der in extra_roots {
        roots.add(CertificateDer::from(der.clone())).map_err(|_| MailError::Server("an extra root certificate is not valid".into()))?;
    }
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions()
        .map_err(|_| MailError::Server("TLS is not available".into()))?
        .with_root_certificates(roots).with_no_client_auth();
    Ok(Arc::new(config))
}

/// The DER certificates in a PEM file, for [`crate::ImapConfig::extra_roots`].
pub fn certificates_from_pem(pem: &str) -> Vec<Vec<u8>> {
    use base64::Engine;
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in pem.lines().map(str::trim) {
        if line == "-----BEGIN CERTIFICATE-----" { current = Some(String::new()); }
        else if line == "-----END CERTIFICATE-----" {
            if let Some(der) = current.take().and_then(|body| base64::engine::general_purpose::STANDARD.decode(body).ok()) { out.push(der); }
        } else if let Some(body) = current.as_mut() { body.push_str(line); }
    }
    out
}

pub async fn connect(endpoint: &Endpoint, limit: Duration) -> Result<TcpStream> {
    let stream = match tokio::time::timeout(limit, TcpStream::connect((endpoint.host.as_str(), endpoint.port))).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => return Err(io_error(&e)),
        Err(_) => return Err(MailError::Network("timed out".into())),
    };
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// TLS over an open socket, verifying the certificate against `host`.
pub async fn secure(config: &Arc<ClientConfig>, host: &str, stream: TcpStream, limit: Duration) -> Result<BoxIo> {
    let name = ServerName::try_from(host.to_owned()).map_err(|_| MailError::Network("the host name is not valid for TLS".into()))?;
    match tokio::time::timeout(limit, TlsConnector::from(config.clone()).connect(name, stream)).await {
        Ok(Ok(stream)) => Ok(Box::new(stream)),
        // A rustls error names what was wrong with the certificate or handshake, nothing about the user.
        Ok(Err(e)) => Err(MailError::Network(format!("TLS: {}", e.get_ref().map_or_else(|| e.kind().to_string(), |inner| inner.to_string())))),
        Err(_) => Err(MailError::Network("timed out".into())),
    }
}

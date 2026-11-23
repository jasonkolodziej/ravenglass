//! SMTP submission (RFC 6409): one connection per message, EHLO, mandatory TLS where configured,
//! AUTH PLAIN / LOGIN / XOAUTH2, then the message. Small enough to own outright; it shares the
//! TLS setup and error mapping with the IMAP side.

use crate::config::{Credential, Endpoint, Security};
use crate::conn::xoauth2_payload;
use crate::net::{self, BoxIo, io_error, server_text};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use mail_core::{MailError, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_rustls::rustls::ClientConfig;

/// A reply line may be at most 512 bytes; allow slack, but never buffer without bound.
const MAX_REPLY_BYTES: usize = 64 * 1024;

pub struct Submission<'a> {
    pub endpoint: &'a Endpoint,
    pub tls: &'a Arc<ClientConfig>,
    pub username: &'a str,
    pub credential: &'a Credential,
    pub timeout: Duration,
}

struct Reply { code: u16, lines: Vec<String> }

impl Reply {
    fn ok(&self) -> bool { (200..400).contains(&self.code) }

    /// The refusal as an error: bad credentials are `Auth`, the rest `Server` with cleaned text.
    fn error(&self, doing: &str) -> MailError {
        if matches!(self.code, 530 | 534 | 535 | 538) { return MailError::Auth; }
        MailError::Server(format!("{doing}: {} {}", self.code, server_text(self.lines.last().map_or("", String::as_str))))
    }
}

struct Link<S> { stream: BufReader<S>, timeout: Duration }

impl<S: AsyncRead + AsyncWrite + Unpin> Link<S> {
    /// Wrap a fresh connection and read the server's greeting.
    async fn greeted(stream: S, timeout: Duration) -> Result<Self> {
        let mut link = Self { stream: BufReader::new(stream), timeout };
        let greeting = link.reply().await?;
        if greeting.ok() { Ok(link) } else { Err(greeting.error("connecting")) }
    }

    async fn reply(&mut self) -> Result<Reply> {
        let mut lines = Vec::new();
        let mut total = 0;
        loop {
            let mut line = Vec::new();
            let read = tokio::time::timeout(self.timeout, self.stream.read_until(b'\n', &mut line)).await
                .map_err(|_| MailError::Network("timed out".into()))?.map_err(|e| io_error(&e))?;
            total += read;
            if read == 0 { return Err(MailError::Network("connection lost".into())); }
            if total > MAX_REPLY_BYTES { return Err(MailError::Server("the SMTP reply is too long".into())); }
            let line = String::from_utf8_lossy(&line).trim_end().to_owned();
            let code = line.get(..3).and_then(|c| c.parse::<u16>().ok()).ok_or_else(|| MailError::Server("the SMTP reply is not valid".into()))?;
            let last = line.as_bytes().get(3) != Some(&b'-');
            lines.push(line.get(4..).unwrap_or("").to_owned());
            if last { return Ok(Reply { code, lines }); }
        }
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let writing = async { self.stream.get_mut().write_all(bytes).await?; self.stream.get_mut().flush().await };
        tokio::time::timeout(self.timeout, writing).await.map_err(|_| MailError::Network("timed out".into()))?.map_err(|e| io_error(&e))
    }

    async fn command(&mut self, line: &str) -> Result<Reply> {
        self.write(format!("{line}\r\n").as_bytes()).await?;
        self.reply().await
    }

    /// Send a command and require a positive reply.
    async fn expect(&mut self, line: &str, doing: &str) -> Result<Reply> {
        let reply = self.command(line).await?;
        if reply.ok() { Ok(reply) } else { Err(reply.error(doing)) }
    }

    /// EHLO, returning the extension lines in upper case. The address literal says nothing
    /// about this machine.
    async fn ehlo(&mut self) -> Result<Vec<String>> {
        Ok(self.expect("EHLO [127.0.0.1]", "greeting").await?.lines.iter().skip(1).map(|l| l.to_ascii_uppercase()).collect())
    }
}

/// Submit `raw` from `from` to `rcpt`. The Bcc header is left out of what goes on the wire.
pub async fn send(submission: Submission<'_>, raw: &[u8], from: &str, rcpt: &[String]) -> Result<()> {
    if rcpt.is_empty() { return Err(MailError::Server("the message has no recipients".into())); }
    for address in std::iter::once(from).chain(rcpt.iter().map(String::as_str)) {
        if address.is_empty() || address.contains(['<', '>', ' ', '\r', '\n']) { return Err(MailError::Server("an address is not valid".into())); }
    }
    let (mut link, extensions) = open(submission).await?;

    let needs_utf8 = !from.is_ascii() || rcpt.iter().any(|r| !r.is_ascii());
    let mut mail_from = format!("MAIL FROM:<{from}>");
    if extensions.iter().any(|e| e == "8BITMIME") && !raw.is_ascii() { mail_from.push_str(" BODY=8BITMIME"); }
    if needs_utf8 {
        if !extensions.iter().any(|e| e == "SMTPUTF8") { return Err(MailError::Server("the server cannot take non-ASCII addresses".into())); }
        mail_from.push_str(" SMTPUTF8");
    }
    link.expect(&mail_from, "sender refused").await?;
    for address in rcpt { link.expect(&format!("RCPT TO:<{address}>"), "recipient refused").await?; }
    link.expect("DATA", "message refused").await?;
    link.write(&wire_format(raw)).await?;
    let accepted = link.reply().await?;
    if !accepted.ok() { return Err(accepted.error("message refused")); }
    // The message is the server's now; a failed goodbye changes nothing.
    let _ = link.command("QUIT").await;
    Ok(())
}

/// Connect, secure and log in, then leave without sending anything. Returns the EHLO extensions.
pub async fn check(submission: Submission<'_>) -> Result<Vec<String>> {
    let (mut link, extensions) = open(submission).await?;
    let _ = link.command("QUIT").await;
    Ok(extensions)
}

/// Connect, secure (as the endpoint says), EHLO, and log in when the server offers AUTH.
async fn open(submission: Submission<'_>) -> Result<(Link<BoxIo>, Vec<String>)> {
    let Submission { endpoint, tls, username, credential, timeout } = submission;
    let tcp = net::connect(endpoint, timeout).await?;
    let mut link: Link<BoxIo> = match endpoint.security {
        Security::Tls => Link::greeted(net::secure(tls, &endpoint.host, tcp, timeout).await?, timeout).await?,
        Security::Plain => Link::greeted(Box::new(tcp) as BoxIo, timeout).await?,
        Security::StartTls => {
            let mut plain = Link::greeted(tcp, timeout).await?;
            // No STARTTLS, no mail: credentials and content never travel in the clear.
            if !plain.ehlo().await?.iter().any(|e| e == "STARTTLS") { return Err(MailError::Server("the server does not offer STARTTLS".into())); }
            plain.expect("STARTTLS", "starting TLS").await?;
            // Anything already buffered was sent before encryption began and cannot be trusted.
            if !plain.stream.buffer().is_empty() { return Err(MailError::Server("unexpected data before TLS".into())); }
            Link { stream: BufReader::new(net::secure(tls, &endpoint.host, plain.stream.into_inner(), timeout).await?), timeout }
        }
    };
    let extensions = link.ehlo().await?;

    let mechanisms: Vec<&str> = extensions.iter().filter_map(|e| e.strip_prefix("AUTH ").or_else(|| e.strip_prefix("AUTH="))).flat_map(str::split_whitespace).collect();
    // A server that offers no AUTH (a local relay, a test server) is used as it is; one that needs
    // a login refuses MAIL FROM itself.
    if !mechanisms.is_empty() { authenticate(&mut link, &mechanisms, username, credential).await?; }
    Ok((link, extensions))
}

async fn authenticate(link: &mut Link<BoxIo>, mechanisms: &[&str], username: &str, credential: &Credential) -> Result<()> {
    let offers = |name: &str| mechanisms.contains(&name);
    let reply = match credential {
        Credential::OAuth2 { access_token } => {
            if !offers("XOAUTH2") { return Err(MailError::Server("the server does not offer XOAUTH2".into())); }
            let reply = link.command(&format!("AUTH XOAUTH2 {}", BASE64.encode(xoauth2_payload(username, access_token)))).await?;
            // 334 carries the error details; an empty line asks for the final verdict.
            if reply.code == 334 { link.command("").await? } else { reply }
        }
        Credential::Password(password) if offers("PLAIN") => link.command(&format!("AUTH PLAIN {}", BASE64.encode(format!("\0{username}\0{password}")))).await?,
        Credential::Password(password) if offers("LOGIN") => {
            let mut reply = link.command("AUTH LOGIN").await?;
            for answer in [username, password.as_str()] {
                if reply.code == 334 { reply = link.command(&BASE64.encode(answer)).await?; }
            }
            reply
        }
        Credential::Password(_) => return Err(MailError::Server("the server offers no supported login method".into())),
    };
    // Any refusal of the AUTH exchange itself is about the credential.
    if reply.code == 235 { Ok(()) } else if (500..600).contains(&reply.code) || reply.code == 334 { Err(MailError::Auth) } else { Err(reply.error("login")) }
}

/// The message as DATA wants it: Bcc removed, CRLF line ends, leading dots doubled, and the
/// closing `.` line.
pub fn wire_format(raw: &[u8]) -> Vec<u8> {
    let message = without_bcc(raw);
    let mut out = Vec::with_capacity(message.len() + message.len() / 40 + 8);
    for line in message.split_inclusive(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\n").map_or(line, |l| l.strip_suffix(b"\r").unwrap_or(l));
        if line.first() == Some(&b'.') { out.push(b'.'); }
        out.extend_from_slice(line);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b".\r\n");
    out
}

/// Drop the Bcc field (and its folded continuation lines) from the header section, so hidden
/// recipients stay hidden. The copy saved to Sent keeps it.
pub fn without_bcc(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut in_headers = true;
    let mut skipping = false;
    for line in raw.split_inclusive(|b| *b == b'\n') {
        if in_headers {
            if line == b"\r\n" || line == b"\n" { in_headers = false; skipping = false; }
            else if matches!(line.first(), Some(b' ' | b'\t')) { if skipping { continue; } }
            else { skipping = line.len() >= 4 && line[..4].eq_ignore_ascii_case(b"bcc:"); if skipping { continue; } }
        }
        out.extend_from_slice(line);
    }
    out
}

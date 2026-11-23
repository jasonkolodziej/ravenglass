//! Helpers for the tests against real servers (Dovecot, Stalwart): finding or starting them,
//! creating a fresh user per test, and an IMAP client, a recording proxy and an SMTP sink that owe
//! nothing to the crate under test.

#![allow(dead_code)]

use mail_core::{AccountId, FolderId};
use mail_imap::{Credential, Endpoint, ImapConfig, Security, certificates_from_pem};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_rustls::rustls;

pub const HOST: &str = "127.0.0.1";
pub const ACCOUNT: &str = "real";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Dovecot, Stalwart }

/// A running server and what the tests need to reach it.
pub struct Server {
    pub kind: Kind,
    /// Its temp dir (users file, certificate, log).
    pub dir: PathBuf,
    /// The PEM of its self-signed certificate.
    pub cert_pem: String,
    pub imap_starttls: u16,
    pub imaps: u16,
    /// Submission with STARTTLS and with implicit TLS (Stalwart only).
    pub submission: Option<(u16, u16)>,
    /// `https://localhost:…/.well-known/jmap` (Stalwart only).
    pub jmap: Option<String>,
}

pub fn tools_dir() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools") }

fn reachable(port: u16) -> bool { TcpStream::connect_timeout(&format!("{HOST}:{port}").parse().unwrap(), Duration::from_secs(1)).is_ok() }

fn bring_up(kind: Kind) -> Option<Server> {
    let (variable, script, ports, name) = match kind {
        Kind::Dovecot => ("RAVENGLASS_DOVECOT_TEST", "dovecot.sh", vec![3144, 3994], "Dovecot"),
        Kind::Stalwart => ("RAVENGLASS_STALWART_TEST", "stalwart.sh", vec![3145, 3995, 3587, 3466, 3443], "Stalwart"),
    };
    if std::env::var(variable).as_deref() != Ok("1") {
        eprintln!("{name} tests skipped. Run them with {variable}=1 (see tools/{script}).");
        return None;
    }
    let pointer = tools_dir().join(format!(".cache/{}.dir", script.trim_end_matches(".sh")));
    if !ports.iter().all(|p| reachable(*p)) || !pointer.exists() {
        let status = std::process::Command::new(tools_dir().join(script)).arg("start").status();
        if !matches!(status, Ok(s) if s.success()) || !ports.iter().all(|p| reachable(*p)) {
            eprintln!("{name} tests skipped: tools/{script} could not start the server.");
            return None;
        }
    }
    let dir = PathBuf::from(std::fs::read_to_string(&pointer).ok()?.trim());
    let cert_pem = std::fs::read_to_string(dir.join("cert.pem")).ok()?;
    Some(match kind {
        Kind::Dovecot => Server { kind, dir, cert_pem, imap_starttls: 3144, imaps: 3994, submission: None, jmap: None },
        Kind::Stalwart => Server { kind, dir, cert_pem, imap_starttls: 3145, imaps: 3995, submission: Some((3587, 3466)), jmap: Some("https://localhost:3443/.well-known/jmap".into()) },
    })
}

/// The Dovecot from `tools/dovecot.sh`, if `RAVENGLASS_DOVECOT_TEST=1`. Says why not, once.
pub fn dovecot() -> Option<&'static Server> {
    static SERVER: OnceLock<Option<Server>> = OnceLock::new();
    SERVER.get_or_init(|| bring_up(Kind::Dovecot)).as_ref()
}

/// The Stalwart from `tools/stalwart.sh`, if `RAVENGLASS_STALWART_TEST=1`.
pub fn stalwart() -> Option<&'static Server> {
    static SERVER: OnceLock<Option<Server>> = OnceLock::new();
    SERVER.get_or_init(|| bring_up(Kind::Stalwart)).as_ref()
}

pub const DOVECOT_PASSWORD: &str = "ravenglass";
pub const STALWART_PASSWORD: &str = "correct-horse-battery-staple-42";

/// A test account: its login and password.
#[derive(Clone, Debug)]
pub struct User { pub login: String, pub password: String, pub address: String }

static USERS: Mutex<()> = Mutex::new(());

fn unique(test: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("{}{:x}", test.replace(|c: char| !c.is_ascii_alphanumeric(), ""), nanos & 0xffff_ffff_ffff)
}

impl Server {
    /// A fresh account. On Dovecot, `inbox_namespace` gives it the `INBOX.` prefix and `.` separator.
    pub fn user_with(&self, test: &str, inbox_namespace: bool) -> User {
        let _one_at_a_time = USERS.lock().unwrap();
        let name = unique(test);
        match self.kind {
            Kind::Dovecot => {
                let extra = if inbox_namespace { "userdb_namespace/inbox/prefix=INBOX. userdb_namespace/inbox/separator=." } else { "" };
                std::fs::write(self.dir.join("users").join(&name), format!("{name}:{{PLAIN}}{DOVECOT_PASSWORD}::::::{extra}\n")).unwrap();
                User { address: format!("{name}@localhost"), login: name, password: DOVECOT_PASSWORD.into() }
            }
            Kind::Stalwart => {
                let out = std::process::Command::new(tools_dir().join("stalwart.sh")).args(["user", &name]).output().unwrap();
                let answer = String::from_utf8_lossy(&out.stdout);
                assert!(out.status.success() && answer.contains("\"created\""), "could not create a Stalwart user: {answer}");
                let address = format!("{name}@ravenglass.test");
                User { login: address.clone(), address, password: STALWART_PASSWORD.into() }
            }
        }
    }

    pub fn user(&self, test: &str) -> User { self.user_with(test, false) }

    pub fn roots(&self) -> Vec<Vec<u8>> { certificates_from_pem(&self.cert_pem) }

    /// Backend settings for `user` over STARTTLS, trusting the server's certificate.
    pub fn config(&self, user: &User) -> ImapConfig {
        let mut config = ImapConfig::new(ACCOUNT, "localhost", "localhost", &user.login, Credential::Password(user.password.clone()));
        config.imap = Endpoint::new("localhost", self.imap_starttls, Security::StartTls);
        if let Some((starttls, _)) = self.submission { config.smtp = Endpoint::new("localhost", starttls, Security::StartTls); }
        config.extra_roots = self.roots();
        config.timeout = Duration::from_secs(30);
        config
    }

    /// Store `raw` in `mailbox` of `user` without the crate under test: `doveadm save` on Dovecot,
    /// IMAP APPEND on Stalwart.
    pub fn deliver_to(&self, user: &User, mailbox: &str, raw: &str) {
        match self.kind {
            Kind::Dovecot => {
                let mut child = std::process::Command::new(tools_dir().join("dovecot.sh"))
                    .args(["doveadm", "save", "-u", &user.login, "-m", mailbox])
                    .stdin(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
                child.stdin.take().unwrap().write_all(raw.as_bytes()).unwrap();
                let out = child.wait_with_output().unwrap();
                assert!(out.status.success(), "doveadm save failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            Kind::Stalwart => { self.imap(user, &[&format!("APPEND {} {{{}+}}\r\n{raw}", quoted(mailbox), raw.len())]); }
        }
    }

    pub fn deliver(&self, user: &User, subject: &str, body: &str) { self.deliver_to(user, "INBOX", &message("sender@example.org", &user.address, subject, body)); }

    /// Log in as `user` over implicit TLS and run `commands`, as another mail client would.
    /// Returns every response line. Panics if a command is refused.
    pub fn imap(&self, user: &User, commands: &[&str]) -> Vec<String> {
        let mut stream = tls_stream(self, self.imaps);
        let mut out = Vec::new();
        let mut reader = LineReader::default();
        reader.line(&mut stream);
        let login = format!("LOGIN {} {}", quoted(&user.login), quoted(&user.password));
        for (i, command) in std::iter::once(login.as_str()).chain(commands.iter().copied()).enumerate() {
            stream.write_all(format!("t{i} {command}\r\n").as_bytes()).unwrap();
            loop {
                let line = reader.line(&mut stream);
                out.push(line.clone());
                if line.starts_with(&format!("t{i} ")) {
                    assert!(line.starts_with(&format!("t{i} OK")), "{}: {line}", command.lines().next().unwrap_or(""));
                    break;
                }
            }
        }
        let _ = stream.write_all(b"z LOGOUT\r\n");
        out
    }
}

pub fn quoted(text: &str) -> String { format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")) }

pub fn message(from: &str, to: &str, subject: &str, body: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("From: Sender <{from}>\r\nTo: {to}\r\nSubject: {subject}\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMessage-ID: <{nanos:x}@test>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n")
}

pub fn account() -> AccountId { AccountId::from(ACCOUNT) }
pub fn folder(mailbox: &str) -> FolderId { FolderId(format!("{ACCOUNT}/{mailbox}")) }
pub fn inbox() -> FolderId { folder("INBOX") }

pub type Tls = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

/// A blocking TLS connection to `port` that trusts only the server's test certificate.
pub fn tls_stream(server: &Server, port: u16) -> Tls {
    let mut roots = rustls::RootCertStore::empty();
    for der in server.roots() { roots.add(rustls::pki_types::CertificateDer::from(der)).unwrap(); }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let connection = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let tcp = TcpStream::connect((HOST, port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    rustls::StreamOwned::new(connection, tcp)
}

/// Reads CRLF lines from a stream, keeping literals (`{n}`) inside the line they belong to.
#[derive(Default)]
pub struct LineReader { buffer: Vec<u8> }

impl LineReader {
    pub fn line(&mut self, stream: &mut impl Read) -> String {
        loop {
            if let Some(end) = self.buffer.windows(2).position(|w| w == b"\r\n") {
                let line: Vec<u8> = self.buffer.drain(..end + 2).collect();
                return String::from_utf8_lossy(&line[..end]).into_owned();
            }
            let mut chunk = [0u8; 16384];
            let n = stream.read(&mut chunk).expect("read from the server");
            assert!(n > 0, "the server closed the connection");
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }
}

/// A TCP relay that records what the client sends, to check which commands a call issued.
/// Plain connections only (Dovecot allows a login without TLS on loopback).
pub struct Recorder { pub port: u16, sent: Arc<Mutex<Vec<u8>>>, open: Arc<Mutex<Vec<TcpStream>>> }

impl Recorder {
    pub fn start(target: u16) -> Self {
        let listener = TcpListener::bind((HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let open = Arc::new(Mutex::new(Vec::new()));
        let (log, shared) = (sent.clone(), open.clone());
        std::thread::spawn(move || {
            for inbound in listener.incoming().flatten() {
                let Ok(outbound) = TcpStream::connect((HOST, target)) else { continue };
                shared.lock().unwrap().extend([inbound.try_clone().unwrap(), outbound.try_clone().unwrap()]);
                let pairs = [(inbound.try_clone().unwrap(), outbound.try_clone().unwrap(), Some(log.clone())), (outbound, inbound, None)];
                for (mut from, mut to, record) in pairs {
                    std::thread::spawn(move || {
                        let mut buffer = [0u8; 65536];
                        while let Ok(n) = from.read(&mut buffer) {
                            if n == 0 { break; }
                            if let Some(record) = &record { record.lock().unwrap().extend_from_slice(&buffer[..n]); }
                            if to.write_all(&buffer[..n]).is_err() { break; }
                        }
                        let _ = to.shutdown(Shutdown::Both);
                    });
                }
            }
        });
        Self { port, sent, open }
    }

    /// The command lines sent since the last call (tags removed), and forget them.
    pub fn take(&self) -> Vec<String> {
        let bytes = std::mem::take(&mut *self.sent.lock().unwrap());
        String::from_utf8_lossy(&bytes).split("\r\n").filter(|l| !l.is_empty())
            .map(|l| l.split_once(' ').map_or(l, |(_, rest)| rest).to_owned()).collect()
    }

    pub fn cut(&self) {
        for stream in self.open.lock().unwrap().drain(..) { let _ = stream.shutdown(Shutdown::Both); }
    }
}

/// An SMTP server that accepts every message and keeps it, for servers without submission.
pub struct SmtpSink { pub port: u16, pub received: Arc<Mutex<Vec<String>>> }

impl SmtpSink {
    pub fn start() -> Self {
        let listener = TcpListener::bind((HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let kept = received.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let kept = kept.clone();
                std::thread::spawn(move || {
                    let mut writer = stream.try_clone().unwrap();
                    let mut reader = BufReader::new(stream);
                    let _ = writer.write_all(b"220 sink\r\n");
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                        let upper = line.to_ascii_uppercase();
                        let reply: &[u8] = if upper.starts_with("EHLO") { b"250-sink\r\n250 8BITMIME\r\n" }
                            else if upper.starts_with("DATA") {
                                let _ = writer.write_all(b"354 go on\r\n");
                                let mut data = String::new();
                                loop {
                                    line.clear();
                                    if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                                    if line == ".\r\n" { break; }
                                    data.push_str(&line);
                                }
                                kept.lock().unwrap().push(data);
                                b"250 kept\r\n"
                            } else if upper.starts_with("QUIT") { let _ = writer.write_all(b"221 bye\r\n"); return; }
                            else { b"250 ok\r\n" };
                        let _ = writer.write_all(reply);
                    }
                });
            }
        });
        Self { port, received }
    }
}

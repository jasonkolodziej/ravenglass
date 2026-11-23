//! Helpers for the GreenMail tests: finding (or starting) the server, a blocking SMTP sender and
//! IMAP script runner that owe nothing to the crate under test, and a TCP proxy that can cut
//! connections.

#![allow(dead_code)]

use mail_core::{AccountId, FolderId};
use mail_imap::{Credential, Endpoint, ImapBackend, ImapConfig, Security};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const HOST: &str = "127.0.0.1";
pub const IMAP: u16 = 3143;
pub const IMAPS: u16 = 3993;
pub const SMTP: u16 = 3025;
pub const SMTPS: u16 = 3465;
pub const ACCOUNT: &str = "gm";

fn reachable() -> bool {
    [IMAP, SMTP].iter().all(|port| TcpStream::connect_timeout(&format!("{HOST}:{port}").parse().unwrap(), Duration::from_secs(1)).is_ok())
}

pub fn tools_dir() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools") }

/// True when the tests should run: `RAVENGLASS_IMAP_TEST=1` and GreenMail answers (it is started
/// with `tools/greenmail.sh start` if it is not already up). Otherwise says why, once.
pub fn available() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| {
        if std::env::var("RAVENGLASS_IMAP_TEST").as_deref() != Ok("1") {
            eprintln!("mail-imap: GreenMail tests skipped. Run them with RAVENGLASS_IMAP_TEST=1 (see tools/greenmail.sh).");
            return false;
        }
        if reachable() { return true; }
        let _ = std::process::Command::new(tools_dir().join("greenmail.sh")).arg("start").status();
        for _ in 0..80 {
            if reachable() { return true; }
            std::thread::sleep(Duration::from_millis(250));
        }
        eprintln!("mail-imap: GreenMail tests skipped: nothing answers on {HOST}:{IMAP} and {HOST}:{SMTP}, and tools/greenmail.sh could not start it (Java 11+ needed).");
        false
    })
}

/// A fresh address. GreenMail creates the account on first delivery, with the address as both
/// login and password.
pub fn unique_user(test: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("{test}-{nanos:x}@localhost")
}

pub fn message(from: &str, to: &str, subject: &str, body: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("From: Sender <{from}>\r\nTo: {to}\r\nSubject: {subject}\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMessage-ID: <{nanos:x}@test>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n")
}

fn smtp_reply(reader: &mut BufReader<TcpStream>) -> String {
    let mut last = String::new();
    loop {
        last.clear();
        assert!(reader.read_line(&mut last).unwrap() > 0, "SMTP connection closed");
        if last.as_bytes().get(3) != Some(&b'-') { return last; }
    }
}

/// Hand `raw` to GreenMail's SMTP port for `to`, without the crate under test.
pub fn deliver(to: &str, raw: &str) {
    let stream = TcpStream::connect((HOST, SMTP)).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    smtp_reply(&mut reader);
    for command in ["EHLO test".to_owned(), "MAIL FROM:<seed@localhost>".to_owned(), format!("RCPT TO:<{to}>"), "DATA".to_owned()] {
        writer.write_all(format!("{command}\r\n").as_bytes()).unwrap();
        let reply = smtp_reply(&mut reader);
        assert!(reply.starts_with('2') || reply.starts_with('3'), "{command}: {reply}");
    }
    writer.write_all(format!("{raw}\r\n.\r\n").as_bytes()).unwrap();
    assert!(smtp_reply(&mut reader).starts_with('2'));
    let _ = writer.write_all(b"QUIT\r\n");
}

/// Log in as `user` on a separate connection and run `commands`, as another mail client would.
/// Returns every response line. Panics if a command is refused.
pub fn imap(user: &str, commands: &[&str]) -> Vec<String> {
    let stream = TcpStream::connect((HOST, IMAP)).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut out = Vec::new();
    let login = format!("LOGIN \"{user}\" \"{user}\"");
    for (i, command) in std::iter::once(login.as_str()).chain(commands.iter().copied()).enumerate() {
        writer.write_all(format!("t{i} {command}\r\n").as_bytes()).unwrap();
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0, "IMAP connection closed");
            out.push(line.trim_end().to_owned());
            if line.starts_with(&format!("t{i} ")) {
                assert!(line.starts_with(&format!("t{i} OK")), "{command}: {line}");
                break;
            }
        }
    }
    out
}

pub fn config(user: &str) -> ImapConfig {
    let mut config = ImapConfig::new(ACCOUNT, HOST, HOST, user, Credential::Password(user.into()));
    config.imap = Endpoint::new(HOST, IMAP, Security::Plain);
    config.smtp = Endpoint::new(HOST, SMTP, Security::Plain);
    config.timeout = Duration::from_secs(15);
    config
}

pub fn account() -> AccountId { AccountId::from(ACCOUNT) }
pub fn folder(mailbox: &str) -> FolderId { FolderId(format!("{ACCOUNT}/{mailbox}")) }
pub fn inbox() -> FolderId { folder("INBOX") }

/// One test's mailbox: a new account holding one message (subject `seed`), and a backend for it.
pub struct Rig { pub user: String, pub backend: ImapBackend }

impl Rig {
    /// `None` (after saying why) when the GreenMail tests are not enabled.
    pub fn new(test: &str) -> Option<Self> { Self::with(test, |_| {}) }

    pub fn with(test: &str, adjust: impl FnOnce(&mut ImapConfig)) -> Option<Self> {
        if !available() { return None; }
        let user = unique_user(test);
        deliver(&user, &message("seed@localhost", &user, "seed", "The first message in the mailbox."));
        let mut config = config(&user);
        adjust(&mut config);
        Some(Self { backend: ImapBackend::new(config).unwrap(), user })
    }

    pub fn deliver(&self, subject: &str, body: &str) { deliver(&self.user, &message("seed@localhost", &self.user, subject, body)); }

    pub fn imap(&self, commands: &[&str]) -> Vec<String> { imap(&self.user, commands) }
}

/// A TCP relay to a local port whose connections can be cut, to play a dropped network.
pub struct Proxy { pub port: u16, open: Arc<Mutex<Vec<TcpStream>>> }

impl Proxy {
    pub fn start(target: u16) -> Self {
        let listener = TcpListener::bind((HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let open = Arc::new(Mutex::new(Vec::new()));
        let shared = open.clone();
        std::thread::spawn(move || {
            for inbound in listener.incoming().flatten() {
                let Ok(outbound) = TcpStream::connect((HOST, target)) else { continue };
                shared.lock().unwrap().extend([inbound.try_clone().unwrap(), outbound.try_clone().unwrap()]);
                for (mut from, mut to) in [(inbound.try_clone().unwrap(), outbound.try_clone().unwrap()), (outbound, inbound)] {
                    std::thread::spawn(move || {
                        let mut buffer = [0u8; 8192];
                        while let Ok(n) = from.read(&mut buffer) {
                            if n == 0 || to.write_all(&buffer[..n]).is_err() { break; }
                        }
                        let _ = to.shutdown(Shutdown::Both);
                    });
                }
            }
        });
        Self { port, open }
    }

    /// Close every connection made so far. New ones still get through.
    pub fn cut(&self) {
        for stream in self.open.lock().unwrap().drain(..) { let _ = stream.shutdown(Shutdown::Both); }
    }
}

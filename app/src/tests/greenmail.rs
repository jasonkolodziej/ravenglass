//! The whole app against a real IMAP/SMTP server (GreenMail). Skipped unless
//! `RAVENGLASS_IMAP_TEST=1`:
//!
//! ```text
//! tools/greenmail.sh start
//! RAVENGLASS_IMAP_TEST=1 cargo test -p ravenglass greenmail
//! tools/greenmail.sh stop
//! ```
//!
//! A temporary profile, in-memory secrets and one account pointing at 127.0.0.1 (the only place
//! an unencrypted connection is allowed). The engine runs on its own thread, as in the window.

use super::profile::TempProfile;
use crate::actions::Action;
use crate::shell::Services;
use crate::state::AppState;
use mail_auth::{AccountConfig, AuthKind, Incoming, MemorySecrets, SecretKind, SecretStore, Security, Server};
use mail_core::AccountId;
use mail_core::machine::account_sync;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HOST: &str = "127.0.0.1";
const IMAP: u16 = 3143;
const SMTP: u16 = 3025;

fn reachable() -> bool {
    [IMAP, SMTP].iter().all(|port| format!("{HOST}:{port}").parse().is_ok_and(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok()))
}

/// True when the test should run: asked for, and GreenMail answers (it is started with
/// `tools/greenmail.sh start` if it is not up). Otherwise says why.
fn available() -> bool {
    if std::env::var("RAVENGLASS_IMAP_TEST").as_deref() != Ok("1") {
        eprintln!("ravenglass: the GreenMail test is skipped. Run it with RAVENGLASS_IMAP_TEST=1 (see tools/greenmail.sh).");
        return false;
    }
    if reachable() { return true; }
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tools/greenmail.sh");
    let _ = std::process::Command::new(script).arg("start").status();
    for _ in 0..80 {
        if reachable() { return true; }
        std::thread::sleep(Duration::from_millis(250));
    }
    eprintln!("ravenglass: the GreenMail test is skipped: nothing answers on {HOST}:{IMAP} and {HOST}:{SMTP}, and tools/greenmail.sh could not start it (Java 11+ needed).");
    false
}

fn nanos() -> u128 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()) }

fn smtp_reply(reader: &mut BufReader<TcpStream>) -> String {
    let mut last = String::new();
    loop {
        last.clear();
        assert!(reader.read_line(&mut last).expect("an SMTP reply") > 0, "the SMTP connection closed");
        if last.as_bytes().get(3) != Some(&b'-') { return last; }
    }
}

/// Hand a message to GreenMail's SMTP port, as another server would. GreenMail creates the
/// account on first delivery, with the address as both login and password.
fn deliver(to: &str, subject: &str, body: &str) {
    let raw = format!("From: Huginn <huginn@localhost>\r\nTo: {to}\r\nSubject: {subject}\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMessage-ID: <{:x}@test>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n", nanos());
    let stream = TcpStream::connect((HOST, SMTP)).expect("the SMTP port");
    let mut writer = stream.try_clone().expect("a second handle");
    let mut reader = BufReader::new(stream);
    smtp_reply(&mut reader);
    for command in ["EHLO test".to_owned(), "MAIL FROM:<huginn@localhost>".to_owned(), format!("RCPT TO:<{to}>"), "DATA".to_owned()] {
        writer.write_all(format!("{command}\r\n").as_bytes()).expect("a write");
        let reply = smtp_reply(&mut reader);
        assert!(reply.starts_with('2') || reply.starts_with('3'), "{command}: {reply}");
    }
    writer.write_all(format!("{raw}\r\n.\r\n").as_bytes()).expect("a write");
    assert!(smtp_reply(&mut reader).starts_with('2'));
    let _ = writer.write_all(b"QUIT\r\n");
}

/// Fold what the engine and job threads send until `done`, as the window's `poll` does.
fn wait_for(s: &mut AppState, what: &str, done: impl Fn(&AppState) -> bool) {
    let start = Instant::now();
    while !done(s) {
        assert!(start.elapsed() < Duration::from_secs(40), "timed out waiting for {what}: sync {:?}, toast {:?}, problems {:?}", s.mail.sync, s.toast.as_ref().map(|t| &t.text), s.shell.problems);
        std::thread::sleep(Duration::from_millis(40));
        s.shell_poll(start.elapsed().as_secs_f64());
        s.pump();
    }
}

fn subjects(s: &AppState) -> Vec<String> { s.rows().iter().map(|e| e.subject.clone()).collect() }

#[test]
fn a_real_account_syncs_reads_and_receives_mail() {
    if !available() { return; }
    let user = format!("app-{:x}@localhost", nanos());
    let id = AccountId::from("greenmail");
    let first = format!("Ferry schedule {:x}", nanos());
    deliver(&user, &first, "The pass is open until Friday.");

    let temp = TempProfile::new("greenmail");
    let account = AccountConfig {
        id: id.clone(),
        name: "Raven".into(),
        address: user.clone(),
        username: user.clone(),
        incoming: Incoming::Imap(Server::new(HOST, IMAP, Security::Plain)),
        smtp: Some(Server::new(HOST, SMTP, Security::Plain)),
        auth: AuthKind::Password,
        server_saves_sent: false,
    };
    temp.profile().save_accounts(&[account]).expect("saved");
    let secrets = Arc::new(MemorySecrets::default());
    secrets.set(&id, SecretKind::Password, &user).expect("the store");

    // The window's path: nothing is connected until the engine thread starts after the first frame.
    let profile = temp.profile();
    let loaded = profile.load();
    let mut s = AppState::real(profile, loaded, Services::offline(secrets), false);
    assert!(s.link.is_off() && s.rows().is_empty());
    s.waker = Some(Arc::new(|| {}));
    s.start_engine();

    wait_for(&mut s, "the first sync", |s| s.mail.sync.get(&id) == Some(&account_sync::State::Idle) && subjects(s).contains(&first));
    assert!(s.shell.problems.is_empty() && s.shell.status_line().is_none());
    let inbox = s.folder().expect("the Inbox").clone();
    assert_eq!((inbox.name.as_str(), inbox.account.clone()), ("Inbox", id.clone()));
    assert!(inbox.total >= 1 && inbox.unread >= 1, "{inbox:?}");
    assert!(crate::view::workspaces::sync_text(&s).starts_with("synced"), "{}", crate::view::workspaces::sync_text(&s));
    assert!(temp.dir.join("mail.db").exists(), "the cache is in the profile");

    // Open it: the body comes from the server.
    s.ws_mut().sel = subjects(&s).iter().position(|subject| subject == &first).expect("the row");
    s.open_selected(true);
    wait_for(&mut s, "the body", |s| s.body().is_some());
    let body = s.body().expect("a body");
    assert_eq!(body.subject, first);
    assert!(body.text.lines.iter().any(|l| l.text().contains("The pass is open until Friday.")), "{:?}", body.text);

    // New mail arrives while the app is open.
    let second = format!("Second crossing {:x}", nanos());
    deliver(&user, &second, "Bring the maps.");
    s.apply(Action::Refresh);
    wait_for(&mut s, "the new message", |s| subjects(s).contains(&second));
    assert!(subjects(&s).contains(&first));

    // A second start reads the cache from the profile before the server answers.
    let link = std::mem::replace(&mut s.link, crate::link::Link::Off);
    link.shutdown();
    let store = mail_store::Store::open(&temp.dir.join("mail.db").to_string_lossy()).expect("the store");
    let folders = store.folders(&id).expect("folders");
    let cached = store.envelopes(&folders.iter().find(|f| f.name == "Inbox").expect("the Inbox").id, 0, 50).expect("envelopes");
    assert!(cached.iter().any(|e| e.subject == first) && cached.iter().any(|e| e.subject == second));
}

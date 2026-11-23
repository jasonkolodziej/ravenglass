//! The `ravenglass-check` binary against Dovecot and Stalwart (IMAP + SMTP, and JMAP), each
//! gated like the backend tests (`RAVENGLASS_DOVECOT_TEST=1`, `RAVENGLASS_STALWART_TEST=1`).
//! Besides passing, the report must hold nothing private: no address, login, password, subject,
//! sender, file name or name of a folder the user made.

#[path = "../../mail-imap/tests/real/mod.rs"]
mod real;

use real::{Server, User, dovecot, stalwart};
use std::process::{Command, Stdio};
use std::time::Duration;

const PRIVATE: [&str; 7] = ["Quarterly numbers", "boss@corp.example", "board-minutes.pdf", "Secret-Project", "hello there", "sender@example.org", "corp.example"];

/// Mail and a folder that the report must not reveal.
fn fill(server: &Server, user: &User) {
    server.imap(user, &["CREATE Secret-Project"]);
    let raw = format!("From: The Boss <boss@corp.example>\r\nTo: {}\r\nSubject: Quarterly numbers\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=m\r\n\r\n--m\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>hello there</p><img src=\"https://cdn.corp.example/logo.png\"><img src=\"https://t.corp.example/open.gif\" width=1 height=1>\r\n--m\r\nContent-Type: application/pdf; name=\"board-minutes.pdf\"\r\nContent-Disposition: attachment; filename=\"board-minutes.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--m--\r\n", user.address);
    server.deliver(user, "older", "x");
    server.deliver_to(user, "INBOX", &raw);
}

/// Run the binary with `args` and the password in the environment; returns its exit status and output.
fn check(user: &User, args: &[String], during: impl FnOnce()) -> (bool, String) {
    let child = Command::new(env!("CARGO_BIN_EXE_ravenglass-check")).args(args).env("RAVENGLASS_CHECK_PASSWORD", &user.password)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    during();
    let out = child.wait_with_output().unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

fn assert_private(report: &str, user: &User) {
    let local = user.login.split('@').next().unwrap();
    for secret in PRIVATE.iter().copied().chain([user.address.as_str(), user.login.as_str(), local, user.password.as_str()]) {
        assert!(!report.contains(secret), "the report shows {secret:?}:\n{report}");
    }
}

fn expect(report: &str, lines: &[&str]) {
    for wanted in lines { assert!(report.contains(wanted), "no {wanted:?} in:\n{report}"); }
}

#[test]
fn check_dovecot_over_imap() {
    let Some(server) = dovecot() else { return };
    let user = server.user("check");
    fill(server, &user);
    let ca = server.dir.join("cert.pem").display().to_string();
    // Dovecot logs in by name; the address only has to look like one.
    let address = format!("{}@dovecot.test", user.login);
    let args: Vec<String> = ["--yes", "--imap", "localhost:3144:starttls", "--smtp", "none", "--login", &user.login, "--ca", &ca, "--idle", "4", "--write", &address].map(String::from).to_vec();
    let (ok, report) = check(&user, &args, || server.deliver(&user, "during the watch", "x"));
    assert!(ok, "{report}");
    expect(&report, &["read the newest 2 of 2 envelopes", "parts: multipart/mixed, text/html, application/pdf", "remote images: 1", "trackers removed: 1", "attachments: 1",
        "other folders: 1", "events: 1 new mail", "body round-trip ok; folder deleted", "all checks passed"]);
    assert_private(&report, &user);
    // The write check left nothing behind, and the read-only part changed no flag.
    let listed = server.imap(&user, &["LIST \"\" \"*\"", "SELECT INBOX", "FETCH 1:* (FLAGS)"]);
    assert!(!listed.iter().any(|l| l.contains("Ravenglass-Check")), "{listed:?}");
    assert!(listed.iter().filter(|l| l.contains("FETCH (FLAGS")).all(|l| !l.contains("\\Seen")), "{listed:?}");
}

#[test]
fn check_stalwart_over_imap_and_smtp() {
    let Some(server) = stalwart() else { return };
    let user = server.user("check");
    fill(server, &user);
    let ca = server.dir.join("cert.pem").display().to_string();
    let args: Vec<String> = ["--yes", "--imap", "localhost:3995:tls", "--smtp", "localhost:3587:starttls", "--ca", &ca, "--idle", "4", "--write", &user.address].map(String::from).to_vec();
    let (ok, report) = check(&user, &args, || { server.imap(&user, &["SELECT INBOX", "STORE 1 +FLAGS (\\Flagged)"]); });
    assert!(ok, "{report}");
    expect(&report, &["smtp         logged in without sending", "Sent: \"Sent Items\"", "read the newest 2 of 2 envelopes", "1 flag changes", "all checks passed"]);
    assert_private(&report, &user);
}

#[test]
fn check_stalwart_over_jmap() {
    let Some(server) = stalwart() else { return };
    let user = server.user("checkj");
    fill(server, &user);
    let ca = server.dir.join("cert.pem").display().to_string();
    let args: Vec<String> = ["--yes", "--jmap", server.jmap.as_deref().unwrap(), "--ca", &ca, "--write", &user.address].map(String::from).to_vec();
    let (ok, report) = check(&user, &args, || {});
    assert!(ok, "{report}");
    expect(&report, &["JMAP https://localhost:3443/.well-known/jmap", "urn:ietf:params:jmap:mail", "read the newest 2 of 2 envelopes", "remote images: 1", "all checks passed"]);
    assert_private(&report, &user);

    // A wrong password fails cleanly, with nothing private in the report either.
    let wrong = User { password: "not-the-password-at-all".into(), ..user.clone() };
    let (ok, report) = check(&wrong, &args, || {});
    assert!(!ok && report.contains("FAIL"), "{report}");
    assert_private(&report, &wrong);
}

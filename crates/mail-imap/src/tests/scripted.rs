//! Tests against a scripted server on a local socket, for what GreenMail does not offer:
//! CONDSTORE, SPECIAL-USE, a server that drops connections, STARTTLS refusal, and the SMTP
//! login mechanisms. The script answers each line with the first rule whose needle the line
//! contains; these check the client's side of the conversation, not a real server's behaviour.

use crate::config::{Credential, Endpoint, ImapConfig, Security};
use crate::smtp::{self, Submission};
use crate::{ImapBackend, net};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use mail_core::*;
use ravenglass_engine::runner::block_on;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Close the connection instead of answering.
const CLOSE: &str = "<close>";
/// Matches the empty line a client sends to abandon a SASL exchange.
const EMPTY_LINE: &str = "<empty>";
/// Matches the lone dot that ends SMTP DATA.
const END_OF_DATA: &str = "<dot>";

struct Rule { needle: &'static str, replies: Vec<String>, used: usize }

/// `replies` are used in turn across matches; the last one repeats.
fn rule(needle: &'static str, replies: &[&str]) -> Rule { Rule { needle, replies: replies.iter().map(|r| r.to_string()).collect(), used: 0 } }

struct Fake { port: u16, seen: Arc<Mutex<Vec<String>>> }

impl Fake {
    /// Every line any client sent, in order.
    fn seen(&self) -> Vec<String> { self.seen.lock().unwrap().clone() }
    fn count(&self, needle: &str) -> usize { self.seen().iter().filter(|l| l.contains(needle)).count() }
}

/// `{tag}` in a reply is the IMAP tag of the command being answered. Lines with no rule get `fallback`.
fn serve(greeting: &'static str, fallback: &'static str, rules: Vec<Rule>) -> Fake {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rules = Arc::new(Mutex::new(rules));
    let (log, shared) = (seen.clone(), rules);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (log, rules) = (log.clone(), shared.clone());
            std::thread::spawn(move || converse(stream, greeting, fallback, &log, &rules));
        }
    });
    Fake { port, seen }
}

fn converse(stream: TcpStream, greeting: &str, fallback: &str, log: &Mutex<Vec<String>>, rules: &Mutex<Vec<Rule>>) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    if writer.write_all(greeting.as_bytes()).is_err() { return; }
    let (mut tag, mut in_data, mut line) = (String::new(), false, String::new());
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
        let text = line.trim_end_matches(['\r', '\n']);
        log.lock().unwrap().push(text.to_owned());
        // Inside SMTP DATA nothing is answered until the closing dot.
        if in_data && text != "." { continue; }
        if let Some(first) = text.split(' ').next() && first.len() == 5 && first.starts_with('A') && first[1..].bytes().all(|b| b.is_ascii_digit()) { tag = first.to_owned(); }
        let reply = {
            let mut rules = rules.lock().unwrap();
            let matches = |needle: &str| match needle { EMPTY_LINE => text.is_empty(), END_OF_DATA => text == ".", _ => text.contains(needle) };
            match rules.iter_mut().find(|r| matches(r.needle)) {
                Some(rule) => { let reply = rule.replies[rule.used.min(rule.replies.len() - 1)].clone(); rule.used += 1; reply }
                None => fallback.to_owned(),
            }
        };
        if reply == CLOSE { return; }
        in_data = reply.starts_with("354");
        if writer.write_all(reply.replace("{tag}", &tag).as_bytes()).is_err() { return; }
    }
}

fn imap_backend(fake: &Fake, adjust: impl FnOnce(&mut ImapConfig)) -> ImapBackend {
    let mut config = ImapConfig::new("Work", "127.0.0.1", "127.0.0.1", "odin@asgard.example", Credential::Password("hunter2".into()));
    config.imap = Endpoint::new("127.0.0.1", fake.port, Security::Plain);
    config.timeout = Duration::from_secs(5);
    adjust(&mut config);
    ImapBackend::new(config).unwrap()
}

fn imap_server(mut rules: Vec<Rule>) -> Fake {
    rules.push(rule("CAPABILITY", &["* CAPABILITY IMAP4rev1 CONDSTORE MOVE UIDPLUS IDLE SPECIAL-USE AUTH=XOAUTH2\r\n{tag} OK done\r\n"]));
    serve("* OK scripted server ready\r\n", "{tag} OK done\r\n", rules)
}

/// One FETCH response as a server sends it for the envelope query, literals and all.
fn fetched(seq: u32, uid: u32, flags: &str, subject: &str, body: &str) -> String {
    let header = format!("From: Huginn <huginn@ravens.example>\r\nSubject: {subject}\r\nMessage-ID: <{uid}@ravens.example>\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\n\r\n");
    format!("* {seq} FETCH (UID {uid} FLAGS ({flags}) INTERNALDATE \"23-Nov-2026 10:00:01 +0000\" BODYSTRUCTURE (\"text\" \"plain\" (\"charset\" \"utf-8\") NIL NIL \"7bit\" {} 1 NIL NIL NIL) BODY[HEADER.FIELDS (FROM SUBJECT MESSAGE-ID DATE)] {{{}}}\r\n{header} BODY[TEXT]<0> {{{}}}\r\n{body})\r\n",
        body.len(), header.len(), body.len())
}

fn selected(exists: u32, uid_next: u32, modseq: u64) -> String {
    format!("* {exists} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY 9] ok\r\n* OK [UIDNEXT {uid_next}] ok\r\n* OK [HIGHESTMODSEQ {modseq}] ok\r\n{{tag}} OK [READ-WRITE] done\r\n")
}

#[test]
fn condstore_fetches_only_what_changed() {
    let all = format!("{}{}{{tag}} OK done\r\n", fetched(1, 1, "\\Seen", "one", "First."), fetched(2, 2, "", "two", "Second."));
    let new = format!("{}{{tag}} OK done\r\n", fetched(2, 3, "", "three", "Third."));
    let fake = imap_server(vec![
        rule("SELECT \"INBOX\" (CONDSTORE)", &[&selected(2, 3, 100), &selected(2, 4, 120)]),
        rule("UID FETCH 1:* (UID FLAGS INTERNALDATE", &[&all]),
        rule("UID FETCH 3:* (UID FLAGS INTERNALDATE", &[&new]),
        rule("UID FETCH 1:2 (UID FLAGS) (CHANGEDSINCE 100)", &["* 1 FETCH (UID 2 FLAGS (\\Seen \\Flagged) MODSEQ (110))\r\n{tag} OK done\r\n"]),
        rule("UID SEARCH UID 1:2", &["* SEARCH 2\r\n{tag} OK done\r\n"]),
        // Anything else the client might try would show up as an unexpected BAD.
        rule("UID ", &["{tag} BAD not in the script\r\n"]),
    ]);
    let backend = imap_backend(&fake, |_| {});
    let inbox = FolderId("Work/INBOX".into());
    let id = |uid: u32| MessageId(format!("Work/INBOX;9;{uid}"));

    let first = block_on(backend.sync_folder(&inbox, &SyncToken::default())).unwrap();
    assert_eq!(first.added.iter().map(|e| (e.id.clone(), e.subject.as_str(), e.preview.as_str(), e.flags)).collect::<Vec<_>>(),
        [(id(1), "one", "First.", Flags::SEEN), (id(2), "two", "Second.", Flags::empty())]);
    assert_eq!(first.added[0].from, Address::new("Huginn", "huginn@ravens.example"));
    assert_eq!(first.token, SyncToken("1;9;3;100;1:2".into()));

    // Since then: 1 was deleted, 2 was flagged, 3 arrived.
    let second = block_on(backend.sync_folder(&inbox, &first.token)).unwrap();
    assert_eq!(second.added.iter().map(|e| e.id.clone()).collect::<Vec<_>>(), [id(3)]);
    assert_eq!(second.changed_flags, vec![(id(2), Flags::SEEN | Flags::FLAGGED)]);
    assert_eq!(second.removed, vec![id(1)]);
    assert_eq!(second.token, SyncToken("1;9;4;120;2:3".into()));

    // Same HIGHESTMODSEQ, UIDNEXT and count: SELECT alone settles it.
    let commands_before = fake.count("UID ");
    let third = block_on(backend.sync_folder(&inbox, &second.token)).unwrap();
    assert_eq!((third.added.len(), third.changed_flags.len(), third.removed.len()), (0, 0, 0));
    assert_eq!(third.token, second.token);
    assert_eq!(fake.count("UID "), commands_before, "no FETCH or SEARCH for an unchanged folder");
    assert_eq!(fake.count("LOGIN"), 1, "one connection for all of it");
}

#[test]
fn an_unparseable_fetch_is_retried_asking_for_less() {
    // This server answers partial fetches the way GreenMail does (no space before the literal)
    // and sends a BODYSTRUCTURE no parser could read.
    let header = "From: Huginn <huginn@ravens.example>\r\nSubject: one\r\n\r\n";
    let plain = format!("* 1 FETCH (UID 1 FLAGS (\\Seen) INTERNALDATE \"23-Nov-2026 10:00:01 +0000\" RFC822.SIZE 300 BODY[HEADER.FIELDS (FROM SUBJECT)] {{{}}}\r\n{header})\r\n{{tag}} OK done\r\n", header.len());
    let fake = imap_server(vec![
        rule("SELECT \"INBOX\"", &["* 1 EXISTS\r\n* OK [UIDVALIDITY 9] ok\r\n* OK [UIDNEXT 2] ok\r\n{tag} OK done\r\n"]),
        rule("BODY.PEEK[TEXT]<0.", &["* 1 FETCH (UID 1 BODY[TEXT]<0>{6}\r\nFirst.)\r\n{tag} OK done\r\n"]),
        rule("BODYSTRUCTURE", &["* 1 FETCH (UID 1 BODYSTRUCTURE ((\"text\" \"plain\" oops) \"mixed\"))\r\n{tag} OK done\r\n"]),
        rule("BODY.PEEK[HEADER.FIELDS", &[&plain]),
        rule("UID FETCH 1 (UID BODY.PEEK[TEXT])", &["* 1 FETCH (UID 1 BODY[TEXT] {6}\r\nFirst.)\r\n{tag} OK done\r\n"]),
    ]);
    let backend = imap_backend(&fake, |c| c.disabled_capabilities = vec!["CONDSTORE".into()]);
    let inbox = FolderId("Work/INBOX".into());
    let delta = block_on(backend.sync_folder(&inbox, &SyncToken::default())).unwrap();
    assert_eq!(delta.added.iter().map(|e| (e.subject.as_str(), e.preview.as_str(), e.flags, e.attachments.len())).collect::<Vec<_>>(), [("one", "First.", Flags::SEEN, 0)]);
    assert_eq!(fake.count("LOGIN"), 3, "two unreadable answers, each costing the connection");
    // What worked is remembered: the next full sync asks the short way at once.
    block_on(backend.sync_folder(&inbox, &SyncToken::default())).unwrap();
    assert_eq!((fake.count("LOGIN"), fake.count("BODYSTRUCTURE")), (3, 2));
}

#[test]
fn condstore_can_be_switched_off() {
    let fake = imap_server(vec![rule("SELECT \"INBOX\"", &["* 0 EXISTS\r\n* OK [UIDVALIDITY 9] ok\r\n* OK [UIDNEXT 1] ok\r\n{tag} OK done\r\n"])]);
    let backend = imap_backend(&fake, |c| c.disabled_capabilities = vec!["condstore".into()]);
    let delta = block_on(backend.sync_folder(&FolderId("Work/INBOX".into()), &SyncToken::default())).unwrap();
    assert_eq!(delta.token, SyncToken("1;9;1;0;".into()));
    assert_eq!(fake.count("(CONDSTORE)"), 0);
}

#[test]
fn special_use_folders_get_their_roles_and_counts() {
    let fake = imap_server(vec![
        rule("LIST \"\" \"*\"", &[concat!(
            "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n",
            "* LIST (\\HasChildren \\Noselect) \"/\" \"[Gmail]\"\r\n",
            "* LIST (\\HasNoChildren \\All) \"/\" \"[Gmail]/All Mail\"\r\n",
            "* LIST (\\HasNoChildren \\Sent) \"/\" \"[Gmail]/Sent Mail\"\r\n",
            "* LIST (\\HasNoChildren \\Drafts) \"/\" \"[Gmail]/Entw&APw-rfe\"\r\n",
            "* LIST (\\HasNoChildren) \"/\" \"Sent\"\r\n",
            "{tag} OK done\r\n")]),
        rule("STATUS \"INBOX\"", &["* STATUS \"INBOX\" (MESSAGES 12 UNSEEN 3)\r\n{tag} OK done\r\n"]),
        rule("STATUS \"Sent\"", &["{tag} NO not now\r\n"]),
        rule("STATUS \"[Gmail]/All Mail\"", &["* STATUS \"[Gmail]/All Mail\" (MESSAGES 900 UNSEEN 3)\r\n{tag} OK done\r\n"]),
    ]);
    let backend = imap_backend(&fake, |_| {});
    let folders = block_on(backend.list_folders()).unwrap();
    let summary: Vec<(&str, &str, FolderRole, u32, u32)> = folders.iter().map(|f| (f.id.0.as_str(), f.name.as_str(), f.role, f.total, f.unread)).collect();
    assert_eq!(summary, [
        ("Work/INBOX", "Inbox", FolderRole::Inbox, 12, 3),
        ("Work/[Gmail]/All Mail", "All Mail", FolderRole::Archive, 900, 3),
        ("Work/[Gmail]/Sent Mail", "Sent Mail", FolderRole::Sent, 0, 0),
        ("Work/[Gmail]/Entw&APw-rfe", "Entwürfe", FolderRole::Drafts, 0, 0),
        // The attribute already named the Sent folder; a refused STATUS leaves the counts at zero.
        ("Work/Sent", "Sent", FolderRole::Other, 0, 0),
    ]);
}

#[test]
fn xoauth2_logs_in_with_the_token_and_a_refusal_is_an_auth_error() {
    let good = BASE64.encode("user=odin@asgard.example\x01auth=Bearer good-token\x01\x01");
    let bad = BASE64.encode("user=odin@asgard.example\x01auth=Bearer expired-token\x01\x01");
    let fake = imap_server(vec![
        rule("AUTHENTICATE XOAUTH2", &["+ \r\n"]),
        rule(Box::leak(good.into_boxed_str()), &["{tag} OK welcome\r\n"]),
        // Gmail's way of refusing: a challenge holding the error, then NO to the empty reply.
        rule(Box::leak(bad.into_boxed_str()), &["+ eyJzdGF0dXMiOiI0MDEifQ==\r\n"]),
        rule(EMPTY_LINE, &["{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n"]),
        rule("LIST", &["* LIST () \"/\" \"INBOX\"\r\n{tag} OK done\r\n"]),
    ]);
    let backend = imap_backend(&fake, |c| c.credential = Credential::OAuth2 { access_token: "expired-token".into() });
    assert_eq!(block_on(backend.list_folders()), Err(MailError::Auth));
    backend.set_credential(Credential::OAuth2 { access_token: "good-token".into() });
    assert_eq!(block_on(backend.list_folders()).unwrap().len(), 1);
    assert_eq!(fake.count("LOGIN"), 0);
    assert!(fake.seen().iter().all(|line| !line.contains("-token")), "the token only travels inside the SASL response");
}

#[test]
fn a_dead_connection_is_replaced_once_and_a_dead_server_is_a_network_error() {
    let fake = imap_server(vec![
        rule("LIST", &["* LIST () \"/\" \"INBOX\"\r\n{tag} OK done\r\n"]),
        rule("SELECT", &[CLOSE, "* 0 EXISTS\r\n* OK [UIDVALIDITY 9] ok\r\n* OK [UIDNEXT 1] ok\r\n{tag} OK done\r\n", CLOSE]),
    ]);
    let backend = imap_backend(&fake, |c| c.disabled_capabilities = vec!["CONDSTORE".into()]);
    let inbox = FolderId("Work/INBOX".into());
    block_on(backend.list_folders()).unwrap();
    // The open connection dies on SELECT; a second one is made and the sync goes through.
    assert!(block_on(backend.sync_folder(&inbox, &SyncToken::default())).is_ok());
    assert_eq!(fake.count("LOGIN"), 2);
    // From now on every SELECT is cut off: one more try, then the error.
    assert_eq!(block_on(backend.sync_folder(&inbox, &SyncToken::default())), Err(MailError::Network("connection lost".into())));
    assert_eq!(fake.count("LOGIN"), 3);
}

#[test]
fn a_server_error_keeps_the_connection_and_hides_addresses() {
    let fake = imap_server(vec![
        rule("SELECT \"Gone\"", &["{tag} NO [NONEXISTENT] Unknown Mailbox: Gone\r\n"]),
        rule("SELECT", &["* 1 EXISTS\r\n* OK [UIDVALIDITY 9] ok\r\n* OK [UIDNEXT 2] ok\r\n{tag} OK done\r\n"]),
        rule("UID STORE", &["{tag} NO [NOPERM] odin@asgard.example may not change flags here\r\n"]),
    ]);
    let backend = imap_backend(&fake, |c| c.disabled_capabilities = vec!["CONDSTORE".into()]);
    assert_eq!(block_on(backend.sync_folder(&FolderId("Work/Gone".into()), &SyncToken::default())), Err(MailError::NotFound("folder".into())));
    let refused = block_on(backend.set_flags(&[MessageId("Work/INBOX;9;1".into())], Flags::SEEN, Flags::empty()));
    assert_eq!(refused, Err(MailError::Server("[NOPERM] <address> may not change flags here".into())));
    // An id from an older UIDVALIDITY is refused before anything is changed.
    let stale = block_on(backend.set_flags(&[MessageId("Work/INBOX;8;1".into())], Flags::SEEN, Flags::empty()));
    assert_eq!(stale, Err(MailError::NotFound("message".into())));
    assert_eq!(fake.count("LOGIN"), 1, "refusals do not cost the connection");
    assert_eq!(fake.count("UID STORE"), 1);
}

#[test]
fn imap_starttls_refused_means_no_login() {
    let fake = imap_server(vec![rule("STARTTLS", &["{tag} BAD unknown command\r\n"])]);
    let backend = imap_backend(&fake, |c| c.imap.security = Security::StartTls);
    assert_eq!(block_on(backend.list_folders()), Err(MailError::Server("the server does not offer STARTTLS".into())));
    assert_eq!(fake.seen().len(), 1, "only STARTTLS was sent: {:?}", fake.seen());
}

#[test]
fn imap_starttls_accepted_goes_straight_to_the_handshake() {
    // The script cannot speak TLS, so the handshake fails; the point is what was sent in the clear.
    let fake = imap_server(vec![]);
    let backend = imap_backend(&fake, |c| c.imap.security = Security::StartTls);
    assert!(matches!(block_on(backend.list_folders()), Err(MailError::Network(_))));
    assert!(fake.seen()[0].ends_with("STARTTLS"));
    assert_eq!(fake.count("LOGIN"), 0);
}

// ── SMTP ─────────────────────────────────────────────────────────────────────────────────────

const RAW: &[u8] = "From: Odin <odin@asgard.example>\r\nTo: muninn@ravens.example\r\nBcc: huginn@ravens.example\r\nSubject: Gr\u{fc}\u{df}e\r\n\r\n.dot\r\n".as_bytes();

fn submit(fake: &Fake, security: Security, credential: &Credential, rcpt: &[&str]) -> Result<()> {
    let endpoint = Endpoint::new("127.0.0.1", fake.port, security);
    let tls = net::tls_config(&[]).unwrap();
    let submission = Submission { endpoint: &endpoint, tls: &tls, username: "odin@asgard.example", credential, timeout: Duration::from_secs(5) };
    let rcpt: Vec<String> = rcpt.iter().map(|r| r.to_string()).collect();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(smtp::send(submission, RAW, "odin@asgard.example", &rcpt))
}

fn smtp_server(ehlo: &'static str, mut rules: Vec<Rule>) -> Fake {
    rules.insert(0, rule("EHLO", &[ehlo]));
    rules.push(rule("DATA", &["354 go ahead\r\n"]));
    serve("220 scripted ESMTP\r\n", "250 OK\r\n", rules)
}

#[test]
fn smtp_sends_with_auth_plain_and_hides_bcc() {
    let fake = smtp_server("250-scripted\r\n250-8BITMIME\r\n250 AUTH LOGIN PLAIN\r\n", vec![rule("AUTH PLAIN", &["235 2.7.0 accepted\r\n"])]);
    submit(&fake, Security::Plain, &Credential::Password("hunter2".into()), &["muninn@ravens.example", "huginn@ravens.example"]).unwrap();
    let plain = BASE64.encode("\0odin@asgard.example\0hunter2");
    assert_eq!(fake.seen(), [
        "EHLO [127.0.0.1]", &format!("AUTH PLAIN {plain}"), "MAIL FROM:<odin@asgard.example> BODY=8BITMIME",
        "RCPT TO:<muninn@ravens.example>", "RCPT TO:<huginn@ravens.example>", "DATA",
        "From: Odin <odin@asgard.example>", "To: muninn@ravens.example", "Subject: Gr\u{fc}\u{df}e", "", "..dot", ".", "QUIT",
    ]);
}

#[test]
fn smtp_falls_back_to_auth_login_and_reports_a_bad_password() {
    let ehlo = "250-scripted\r\n250 AUTH LOGIN\r\n";
    let (user, good, bad) = (BASE64.encode("odin@asgard.example"), BASE64.encode("hunter2"), BASE64.encode("wrong"));
    let rules = || vec![
        rule("AUTH LOGIN", &["334 VXNlcm5hbWU6\r\n"]),
        rule(Box::leak(user.clone().into_boxed_str()), &["334 UGFzc3dvcmQ6\r\n"]),
        rule(Box::leak(good.clone().into_boxed_str()), &["235 2.7.0 accepted\r\n"]),
        rule(Box::leak(bad.clone().into_boxed_str()), &["535 5.7.8 Username and Password not accepted\r\n"]),
    ];
    let fake = smtp_server(ehlo, rules());
    assert_eq!(submit(&fake, Security::Plain, &Credential::Password("hunter2".into()), &["muninn@ravens.example"]), Ok(()));
    let fake = smtp_server(ehlo, rules());
    assert_eq!(submit(&fake, Security::Plain, &Credential::Password("wrong".into()), &["muninn@ravens.example"]), Err(MailError::Auth));
    assert_eq!(fake.count("MAIL FROM"), 0);
}

#[test]
fn smtp_xoauth2_succeeds_or_is_an_auth_error() {
    let ehlo = "250-scripted\r\n250 AUTH PLAIN XOAUTH2\r\n";
    let good = BASE64.encode("user=odin@asgard.example\x01auth=Bearer good-token\x01\x01");
    let rules = || vec![
        rule(Box::leak(format!("AUTH XOAUTH2 {good}").into_boxed_str()), &["235 2.7.0 accepted\r\n"]),
        rule("AUTH XOAUTH2", &["334 eyJzdGF0dXMiOiI0MDEifQ==\r\n"]),
        rule(EMPTY_LINE, &["535 5.7.8 Username and Password not accepted\r\n"]),
    ];
    let fake = smtp_server(ehlo, rules());
    assert_eq!(submit(&fake, Security::Plain, &Credential::OAuth2 { access_token: "good-token".into() }, &["muninn@ravens.example"]), Ok(()));
    assert_eq!(fake.count("AUTH PLAIN"), 0);
    let fake = smtp_server(ehlo, rules());
    assert_eq!(submit(&fake, Security::Plain, &Credential::OAuth2 { access_token: "expired".into() }, &["muninn@ravens.example"]), Err(MailError::Auth));
    // A server with no XOAUTH2 is not handed the token some other way.
    let fake = smtp_server("250-scripted\r\n250 AUTH PLAIN LOGIN\r\n", vec![]);
    assert!(matches!(submit(&fake, Security::Plain, &Credential::OAuth2 { access_token: "good-token".into() }, &["muninn@ravens.example"]), Err(MailError::Server(_))));
    assert_eq!(fake.count("AUTH"), 0);
}

#[test]
fn smtp_refusals_name_the_step_but_not_the_address() {
    let fake = smtp_server("250 scripted\r\n", vec![rule("RCPT TO:<nobody@ravens.example>", &["550 5.1.1 <nobody@ravens.example>: Recipient address rejected\r\n"])]);
    let refused = submit(&fake, Security::Plain, &Credential::Password("hunter2".into()), &["muninn@ravens.example", "nobody@ravens.example"]);
    assert_eq!(refused, Err(MailError::Server("recipient refused: 550 5.1.1 <address> Recipient address rejected".into())));
    // No AUTH on offer: none attempted, and nothing was sent as DATA after the refusal.
    assert_eq!((fake.count("AUTH"), fake.count("DATA")), (0, 0));

    let fake = smtp_server("250 scripted\r\n", vec![rule(END_OF_DATA, &["552 5.3.4 Message too big\r\n"])]);
    let refused = submit(&fake, Security::Plain, &Credential::Password("hunter2".into()), &["muninn@ravens.example"]);
    assert_eq!(refused, Err(MailError::Server("message refused: 552 5.3.4 Message too big".into())));

    let fake = smtp_server("250 scripted\r\n", vec![]);
    assert!(matches!(submit(&fake, Security::Plain, &Credential::Password("x".into()), &["two words@x"]), Err(MailError::Server(_))));
    assert!(fake.seen().is_empty(), "a bad address is caught before connecting");
}

#[test]
fn smtp_starttls_is_never_skipped() {
    // Not offered: stop before any credential or mail.
    let fake = smtp_server("250-scripted\r\n250 AUTH PLAIN\r\n", vec![]);
    let refused = submit(&fake, Security::StartTls, &Credential::Password("hunter2".into()), &["muninn@ravens.example"]);
    assert_eq!(refused, Err(MailError::Server("the server does not offer STARTTLS".into())));
    assert_eq!(fake.seen(), ["EHLO [127.0.0.1]"]);
    // Offered: the next thing on the wire is the TLS handshake (which the script cannot answer).
    let fake = smtp_server("250-scripted\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n", vec![rule("STARTTLS", &["220 go ahead\r\n"])]);
    assert!(matches!(submit(&fake, Security::StartTls, &Credential::Password("hunter2".into()), &["muninn@ravens.example"]), Err(MailError::Network(_))));
    assert_eq!(fake.count("AUTH"), 0);
    assert_eq!(&fake.seen()[..2], ["EHLO [127.0.0.1]", "STARTTLS"]);
}

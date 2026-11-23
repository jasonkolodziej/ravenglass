//! `ImapBackend` against a real server (GreenMail). Skipped unless `RAVENGLASS_IMAP_TEST=1`:
//!
//! ```text
//! tools/greenmail.sh start
//! RAVENGLASS_IMAP_TEST=1 cargo test -p mail-imap
//! ```
//!
//! Every future is driven by the engine's own `runner::block_on`, a thread-parking loop with no
//! tokio runtime, which is how the desktop app calls the backend.

mod support;

use mail_core::*;
use mail_imap::{Credential, Endpoint, ImapBackend, Security, certificates_from_pem};
use ravenglass_engine::runner::block_on;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};
use support::*;

fn full_sync(backend: &ImapBackend, folder: &FolderId) -> FolderDelta { block_on(backend.sync_folder(folder, &SyncToken::default())).unwrap() }

fn find<'a>(delta: &'a FolderDelta, subject: &str) -> &'a Envelope {
    delta.added.iter().find(|e| e.subject == subject).unwrap_or_else(|| panic!("no message with subject {subject:?} among {:?}", delta.added.iter().map(|e| &e.subject).collect::<Vec<_>>()))
}

#[test]
fn a_wrong_password_is_an_auth_error() {
    let Some(rig) = Rig::new("auth") else { return };
    let mut config = config(&rig.user);
    config.credential = Credential::Password("not the password".into());
    let backend = ImapBackend::new(config).unwrap();
    assert_eq!(block_on(backend.list_folders()), Err(MailError::Auth));
    // The right one, handed over later (as a refreshed token would be), is picked up on the next connect.
    backend.set_credential(Credential::Password(rig.user.clone()));
    assert!(block_on(backend.list_folders()).is_ok());
}

#[test]
fn an_unreachable_server_is_a_network_error() {
    if !available() { return; }
    let mut config = config("nobody@localhost");
    config.imap = Endpoint::new(HOST, 1, Security::Plain);
    let backend = ImapBackend::new(config).unwrap();
    assert!(matches!(block_on(backend.list_folders()), Err(MailError::Network(_))));
}

#[test]
fn folders_are_listed_with_roles_paths_and_counts() {
    let Some(rig) = Rig::new("folders") else { return };
    rig.imap(&["CREATE \"Trash\"", "CREATE \"Spam\"", "CREATE \"Archive\"", "CREATE \"Sent Items\"", "CREATE \"Receipts.2026\"", "CREATE \"Receipts.Drafts\""]);
    rig.deliver("second", "Another one.");
    rig.imap(&["SELECT INBOX", "UID STORE 1 +FLAGS (\\Seen)"]);

    let folders = block_on(rig.backend.list_folders()).unwrap();
    let by_id = |mailbox: &str| folders.iter().find(|f| f.id == folder(mailbox)).unwrap_or_else(|| panic!("{mailbox} not listed: {folders:?}"));
    let inbox = by_id("INBOX");
    assert_eq!((inbox.role, inbox.name.as_str(), inbox.total, inbox.unread), (FolderRole::Inbox, "Inbox", 2, 1));
    assert_eq!(inbox.account, account());
    assert_eq!(by_id("Trash").role, FolderRole::Trash);
    assert_eq!(by_id("Spam").role, FolderRole::Junk);
    assert_eq!(by_id("Archive").role, FolderRole::Archive);
    assert_eq!(by_id("Sent Items").role, FolderRole::Sent);
    let nested = by_id("Receipts.2026");
    assert_eq!((nested.role, nested.name.as_str(), nested.path.clone()), (FolderRole::Other, "2026", vec!["Receipts".to_owned(), "2026".to_owned()]));
    // A nested folder that merely shares a well-known name still gets the role when nothing better has it.
    assert_eq!(by_id("Receipts.Drafts").role, FolderRole::Drafts);
}

#[test]
fn a_sent_message_arrives_and_a_full_sync_sees_it() {
    let Some(rig) = Rig::new("send") else { return };
    let me = Address::new("Me", &rig.user);
    let raw = format!("From: Me <{0}>\r\nTo: Me <{0}>\r\nCc: Carol <carol@localhost>\r\nBcc: hidden@localhost\r\nSubject: =?utf-8?q?Gr=C3=BC=C3=9Fe?= from Ravenglass\r\nDate: Mon, 23 Nov 2026 12:30:00 +0000\r\nMessage-ID: <sent-1@test>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello there.\r\n.a line that starts with a dot\r\nBye.\r\n", rig.user);
    block_on(rig.backend.send(raw.as_bytes(), &me, std::slice::from_ref(&me))).unwrap();

    let delta = full_sync(&rig.backend, &inbox());
    assert_eq!(delta.added.len(), 2, "the seed and the sent message");
    assert!(delta.changed_flags.is_empty() && delta.removed.is_empty() && !delta.token.0.is_empty());
    let got = find(&delta, "Grüße from Ravenglass");
    assert_eq!(got.from, me);
    assert_eq!(got.to, vec![me.clone()]);
    assert_eq!(got.cc, vec![Address::new("Carol", "carol@localhost")]);
    assert_eq!(got.date, 1_795_437_000);
    assert_eq!(got.thread, ThreadId("sent-1@test".into()));
    assert_eq!(got.folder, inbox());
    assert!(got.is_unread());
    assert_eq!(got.preview, "Hello there. .a line that starts with a dot Bye.");
    // Hidden recipients are not in what went over SMTP…
    let delivered = String::from_utf8(block_on(rig.backend.fetch_body(&got.id)).unwrap().bytes).unwrap();
    assert!(!delivered.contains("hidden@localhost") && delivered.contains("\r\n.a line that starts with a dot\r\n"), "{delivered}");

    // …but the copy saved to Sent (created on demand here) is the message as written, already read.
    let sent = block_on(rig.backend.list_folders()).unwrap().into_iter().find(|f| f.role == FolderRole::Sent).expect("a Sent folder");
    let copy = full_sync(&rig.backend, &sent.id);
    assert_eq!(copy.added.len(), 1);
    assert!(copy.added[0].flags.contains(Flags::SEEN));
    assert_eq!(block_on(rig.backend.fetch_body(&copy.added[0].id)).unwrap().bytes, raw.as_bytes());
}

#[test]
fn save_sent_off_leaves_the_sent_folder_to_the_server() {
    let Some(rig) = Rig::with("nosent", |c| c.save_sent = false) else { return };
    let me = Address::new("Me", &rig.user);
    block_on(rig.backend.send(message(&rig.user, &rig.user, "no copy", "x").as_bytes(), &me, std::slice::from_ref(&me))).unwrap();
    assert!(block_on(rig.backend.list_folders()).unwrap().iter().all(|f| f.role != FolderRole::Sent));
}

#[test]
fn an_incremental_sync_reports_new_mail_flag_changes_and_removals() {
    let Some(rig) = Rig::new("incr") else { return };
    rig.deliver("stays", "This one gets read elsewhere.");
    rig.deliver("goes", "This one is deleted elsewhere.");
    let first = full_sync(&rig.backend, &inbox());
    assert_eq!(first.added.len(), 3);
    let (stays, goes) = (find(&first, "stays").id.clone(), find(&first, "goes").id.clone());

    // Nothing happened: nothing is reported, flags included.
    let quiet = block_on(rig.backend.sync_folder(&inbox(), &first.token)).unwrap();
    assert_eq!((quiet.added.len(), quiet.changed_flags.len(), quiet.removed.len()), (0, 0, 0));

    // Another client reads one message and deletes another; a new one arrives.
    rig.imap(&["SELECT INBOX", "UID STORE 2 +FLAGS (\\Seen \\Flagged)", "UID STORE 3 +FLAGS (\\Deleted)", "EXPUNGE"]);
    rig.deliver("new", "Arrived after the first sync.");
    let second = block_on(rig.backend.sync_folder(&inbox(), &quiet.token)).unwrap();
    assert_eq!(second.added.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["new"]);
    assert_eq!(second.changed_flags, vec![(stays.clone(), Flags::SEEN | Flags::FLAGGED)]);
    assert_eq!(second.removed, vec![goes]);
    assert_ne!(second.token, first.token);

    // A backend that has just started has no memory of flags, so it reports them for every
    // known message (and nothing else): correct, just not minimal.
    let restarted = ImapBackend::new(config(&rig.user)).unwrap();
    let third = block_on(restarted.sync_folder(&inbox(), &second.token)).unwrap();
    assert!(third.added.is_empty() && third.removed.is_empty());
    assert_eq!(third.changed_flags.len(), 3);
    assert!(third.changed_flags.contains(&(stays, Flags::SEEN | Flags::FLAGGED)));

    // A token from an older sync still works: it is the server's state that is compared.
    let replay = block_on(restarted.sync_folder(&inbox(), &first.token)).unwrap();
    assert_eq!((replay.added.len(), replay.removed.len()), (1, 1));
}

#[test]
fn a_recreated_folder_resyncs_and_retires_the_old_ids() {
    let Some(rig) = Rig::new("validity") else { return };
    rig.imap(&["CREATE \"Scratch\"", "SELECT INBOX", "UID COPY 1 \"Scratch\""]);
    let before = full_sync(&rig.backend, &folder("Scratch"));
    assert_eq!(before.added.len(), 1);
    // Deleting and recreating a mailbox gives it a new UIDVALIDITY.
    std::thread::sleep(Duration::from_millis(1100));
    rig.imap(&["DELETE \"Scratch\"", "CREATE \"Scratch\"", "SELECT INBOX", "UID COPY 1 \"Scratch\""]);
    let after = block_on(rig.backend.sync_folder(&folder("Scratch"), &before.token)).unwrap();
    assert_eq!(after.removed, vec![before.added[0].id.clone()]);
    assert_eq!(after.added.len(), 1);
    assert_ne!(after.added[0].id, before.added[0].id, "ids include UIDVALIDITY");
    // The old id no longer names anything.
    assert!(matches!(block_on(rig.backend.fetch_body(&before.added[0].id)), Err(MailError::NotFound(_))));
}

#[test]
fn a_body_is_fetched_whole_and_a_missing_one_is_not_found() {
    let Some(rig) = Rig::new("body") else { return };
    let raw = message("seed@localhost", &rig.user, "body", &"A fairly long line of body text. ".repeat(400));
    deliver(&rig.user, &raw);
    let delta = full_sync(&rig.backend, &inbox());
    let envelope = find(&delta, "body");
    assert_eq!(envelope.preview.chars().count(), 200);
    assert!(envelope.preview.starts_with("A fairly long line of body text. A fairly"));
    let body = block_on(rig.backend.fetch_body(&envelope.id)).unwrap();
    assert_eq!(body.id, envelope.id);
    let text = String::from_utf8(body.bytes).unwrap();
    assert!(text.ends_with(raw.split_once("\r\n\r\n").unwrap().1), "the whole body, byte for byte");
    assert!(text.contains("Subject: body\r\n"));

    let missing = MessageId(envelope.id.0.rsplit_once(';').unwrap().0.to_owned() + ";999");
    assert!(matches!(block_on(rig.backend.fetch_body(&missing)), Err(MailError::NotFound(_))));
    assert!(matches!(block_on(rig.backend.fetch_body(&MessageId("gm/No Such Folder;1;1".into()))), Err(MailError::NotFound(_))));
    assert!(matches!(block_on(rig.backend.fetch_body(&MessageId("other-account/INBOX;1;1".into()))), Err(MailError::NotFound(_))));
    assert!(matches!(block_on(rig.backend.sync_folder(&folder("No Such Folder"), &SyncToken::default())), Err(MailError::NotFound(_))));
    // The connection survives all of those refusals.
    assert!(block_on(rig.backend.fetch_body(&envelope.id)).is_ok());
}

#[test]
fn attachments_are_listed_from_the_structure() {
    let Some(rig) = Rig::new("attach") else { return };
    let raw = format!("From: A <a@localhost>\r\nTo: {}\r\nSubject: with file\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=frontier\r\n\r\n--frontier\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>See the <b>attached</b> route.</p></body></html>\r\n--frontier\r\nContent-Type: application/pdf; name=\"route-north.pdf\"\r\nContent-Disposition: attachment; filename=\"route-north.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQKJcfsj6IKNSAwIG9iago8PC9MZW5ndGggNiAwIFI+PgpzdHJlYW0K\r\n--frontier--\r\n", rig.user);
    deliver(&rig.user, &raw);
    let delta = full_sync(&rig.backend, &inbox());
    let envelope = find(&delta, "with file");
    assert_eq!(envelope.preview, "See the attached route.");
    assert_eq!(envelope.attachments.len(), 1, "{:?}", envelope.attachments);
    let file = &envelope.attachments[0];
    assert_eq!((file.name.as_str(), file.mime.as_str()), ("route-north.pdf", "application/pdf"));
    // The size is the server's count of the encoded part (GreenMail includes the part's headers), so only roughly 48.
    assert!((30..=400).contains(&file.size), "{}", file.size);
    assert!(find(&delta, "seed").attachments.is_empty());
}

#[test]
fn flags_are_set_and_cleared() {
    let Some(rig) = Rig::new("flags") else { return };
    rig.deliver("other", "Left alone.");
    let first = full_sync(&rig.backend, &inbox());
    let seed = find(&first, "seed").id.clone();
    block_on(rig.backend.set_flags(std::slice::from_ref(&seed), Flags::SEEN | Flags::FLAGGED | Flags::ANSWERED, Flags::empty())).unwrap();
    let second = block_on(rig.backend.sync_folder(&inbox(), &first.token)).unwrap();
    assert_eq!(second.changed_flags, vec![(seed.clone(), Flags::SEEN | Flags::FLAGGED | Flags::ANSWERED)]);
    // The engine restores flags with `add = before, remove = !before`, bits IMAP has no name for included.
    block_on(rig.backend.set_flags(std::slice::from_ref(&seed), Flags::SEEN, Flags(!Flags::SEEN.0))).unwrap();
    let third = block_on(rig.backend.sync_folder(&inbox(), &second.token)).unwrap();
    assert_eq!(third.changed_flags, vec![(seed, Flags::SEEN)]);
    assert!(rig.imap(&["SELECT INBOX", "UID FETCH 1:* (FLAGS)"]).iter().any(|l| l.contains("FLAGS (\\Seen)")));
}

/// Move the seed message to Archive and check both folders, with the given capabilities ignored.
fn moves_a_message(test: &str, disabled: &[&str]) {
    let disabled: Vec<String> = disabled.iter().map(|s| s.to_string()).collect();
    let Some(rig) = Rig::with(test, |c| c.disabled_capabilities = disabled) else { return };
    rig.imap(&["CREATE \"Archive\""]);
    rig.deliver("stays", "Not moved.");
    let first = full_sync(&rig.backend, &inbox());
    let seed = find(&first, "seed").id.clone();
    block_on(rig.backend.move_to(std::slice::from_ref(&seed), &folder("Archive"))).unwrap();

    let after = block_on(rig.backend.sync_folder(&inbox(), &first.token)).unwrap();
    assert_eq!(after.removed, vec![seed.clone()]);
    assert!(after.added.is_empty());
    let archive = full_sync(&rig.backend, &folder("Archive"));
    assert_eq!(archive.added.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["seed"]);
    assert_ne!(archive.added[0].id, seed, "a moved message has a new id");
    assert_eq!(rig.imap(&["SELECT INBOX", "UID SEARCH ALL"]).iter().filter(|l| l.starts_with("* SEARCH")).collect::<Vec<_>>(), ["* SEARCH 2"]);

    // And back, which is how the engine undoes a move.
    block_on(rig.backend.move_to(std::slice::from_ref(&archive.added[0].id), &inbox())).unwrap();
    assert_eq!(block_on(rig.backend.sync_folder(&inbox(), &after.token)).unwrap().added.len(), 1);
    // A folder that does not exist refuses the move without breaking anything.
    let stays = find(&first, "stays").id.clone();
    assert!(matches!(block_on(rig.backend.move_to(std::slice::from_ref(&stays), &folder("Nowhere"))), Err(MailError::Server(_))));
    assert!(block_on(rig.backend.fetch_body(&stays)).is_ok());
}

#[test]
fn move_uses_the_move_extension() { moves_a_message("move", &[]); }

#[test]
fn move_falls_back_to_copy_and_uid_expunge() { moves_a_message("copy", &["MOVE"]); }

#[test]
fn move_falls_back_to_copy_and_plain_expunge() { moves_a_message("expunge", &["MOVE", "UIDPLUS"]); }

#[test]
fn a_draft_is_appended_to_drafts() {
    let Some(rig) = Rig::new("draft") else { return };
    let raw = message(&rig.user, "someone@localhost", "unfinished", "Dear someone,");
    // The account has no Drafts folder yet, so one is created.
    let id = block_on(rig.backend.append_draft(raw.as_bytes())).unwrap();
    assert_eq!(block_on(rig.backend.fetch_body(&id)).unwrap().bytes, raw.as_bytes());
    let drafts = block_on(rig.backend.list_folders()).unwrap().into_iter().find(|f| f.role == FolderRole::Drafts).expect("a Drafts folder");
    assert_eq!((drafts.total, drafts.unread), (1, 0));
    let delta = full_sync(&rig.backend, &drafts.id);
    assert_eq!(delta.added.len(), 1);
    assert_eq!(delta.added[0].id, id);
    assert!(delta.added[0].flags.contains(Flags::DRAFT));
    // A second draft gets its own id.
    let second = block_on(rig.backend.append_draft(message(&rig.user, "someone@localhost", "another", "…").as_bytes())).unwrap();
    assert_ne!(second, id);
    assert_eq!(block_on(rig.backend.sync_folder(&drafts.id, &delta.token)).unwrap().added[0].id, second);
}

/// Watch the inbox on a thread; new mail must produce `Exists`, and dropping the receiver must
/// end the watch at the next event.
fn idle_reports_new_mail(test: &str, disabled: &[&str]) {
    let disabled: Vec<String> = disabled.iter().map(|s| s.to_string()).collect();
    let Some(rig) = Rig::with(test, |c| { c.disabled_capabilities = disabled; c.idle_poll = Duration::from_millis(300); }) else { return };
    let Rig { user, backend } = rig;
    let backend = Arc::new(backend);
    let (events, received) = channel();
    let watcher = { let backend = backend.clone(); std::thread::spawn(move || block_on(backend.idle(&inbox(), events))) };
    std::thread::sleep(Duration::from_millis(1500));
    deliver(&user, &message("seed@localhost", &user, "wake up", "New mail."));
    assert_eq!(received.recv_timeout(Duration::from_secs(15)), Ok(IdleEvent::Exists(inbox())));
    // The command connection is separate and works while the watch is on.
    assert_eq!(full_sync(&backend, &inbox()).added.len(), 2);

    drop(received);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !watcher.is_finished() {
        assert!(Instant::now() < deadline, "the watch did not stop after its receiver was dropped");
        deliver(&user, &message("seed@localhost", &user, "anyone there?", "No."));
        std::thread::sleep(Duration::from_millis(500));
    }
    assert_eq!(watcher.join().unwrap(), Ok(()));
}

#[test]
fn idle_wakes_on_new_mail() { idle_reports_new_mail("idle", &[]); }

#[test]
fn without_idle_the_watch_polls() { idle_reports_new_mail("poll", &["IDLE"]); }

#[test]
fn a_dropped_connection_is_reopened_once() {
    if !available() { return; }
    let proxy = Proxy::start(IMAP);
    let Some(rig) = Rig::with("drop", |c| c.imap = Endpoint::new(HOST, proxy.port, Security::Plain)) else { return };
    let first = full_sync(&rig.backend, &inbox());
    proxy.cut();
    // The pooled connection is dead; the call notices, reconnects and succeeds.
    let again = block_on(rig.backend.sync_folder(&inbox(), &first.token)).unwrap();
    assert!(again.added.is_empty() && again.removed.is_empty());

    // A watch that loses its connection comes back and asks the listener to look again.
    let Rig { user, backend } = rig;
    let backend = Arc::new(backend);
    let (events, received) = channel();
    let watcher = { let backend = backend.clone(); std::thread::spawn(move || block_on(backend.idle(&inbox(), events))) };
    std::thread::sleep(Duration::from_millis(1500));
    proxy.cut();
    assert_eq!(received.recv_timeout(Duration::from_secs(15)), Ok(IdleEvent::Exists(inbox())), "catch-up after the reconnect");
    deliver(&user, &message("seed@localhost", &user, "after the cut", "Still watching."));
    assert_eq!(received.recv_timeout(Duration::from_secs(15)), Ok(IdleEvent::Exists(inbox())));
    drop(watcher);
}

#[test]
fn tls_works_with_a_trusted_certificate_and_fails_without() {
    if !available() { return; }
    let Ok(pem) = std::fs::read_to_string(tools_dir().join(".cache/greenmail-tls.pem")) else {
        eprintln!("mail-imap: TLS test skipped: no tools/.cache/greenmail-tls.pem (the server was not started by tools/greenmail.sh)");
        return;
    };
    let secure = |c: &mut mail_imap::ImapConfig| {
        c.imap = Endpoint::new(HOST, IMAPS, Security::Tls);
        c.smtp = Endpoint::new(HOST, SMTPS, Security::Tls);
    };
    let Some(rig) = Rig::with("tls", |c| { secure(c); c.extra_roots = certificates_from_pem(&pem); }) else { return };
    assert_eq!(rig.backend.account(), &account());
    let me = Address::new("Me", &rig.user);
    block_on(rig.backend.send(message(&rig.user, &rig.user, "over TLS", "Encrypted both ways.").as_bytes(), &me, std::slice::from_ref(&me))).unwrap();
    let delta = full_sync(&rig.backend, &inbox());
    assert_eq!(find(&delta, "over TLS").preview, "Encrypted both ways.");

    // The same server without its certificate in the trust store: refused, for IMAP and SMTP alike.
    let mut untrusted = config(&rig.user);
    secure(&mut untrusted);
    let backend = ImapBackend::new(untrusted).unwrap();
    let refused = block_on(backend.list_folders()).unwrap_err();
    assert!(matches!(&refused, MailError::Network(text) if text.starts_with("TLS:")), "{refused:?}");
    let refused = block_on(backend.send(b"Subject: x\r\n\r\nx\r\n", &me, std::slice::from_ref(&me))).unwrap_err();
    assert!(matches!(&refused, MailError::Network(text) if text.starts_with("TLS:")), "{refused:?}");

    // STARTTLS is required when asked for. GreenMail offers it on neither port, so both refuse
    // rather than carry on in the clear.
    let mut starttls = config(&rig.user);
    starttls.imap.security = Security::StartTls;
    starttls.smtp.security = Security::StartTls;
    let backend = ImapBackend::new(starttls).unwrap();
    assert_eq!(block_on(backend.list_folders()), Err(MailError::Server("the server does not offer STARTTLS".into())));
    assert_eq!(block_on(backend.send(b"Subject: x\r\n\r\nx\r\n", &me, std::slice::from_ref(&me))), Err(MailError::Server("the server does not offer STARTTLS".into())));
}

#[test]
fn an_oauth_token_logs_in_over_xoauth2() {
    // GreenMail checks a bearer token against the account's password (here, its address), which
    // is enough to run the real SASL exchange on both protocols. It takes the IMAP token only as
    // a SASL-IR initial response.
    let Some(rig) = Rig::new("oauth") else { return };
    let me = Address::new("Me", &rig.user);
    let mut config = config(&rig.user);
    config.credential = Credential::OAuth2 { access_token: "an expired token".into() };
    let backend = ImapBackend::new(config).unwrap();
    assert_eq!(block_on(backend.list_folders()), Err(MailError::Auth));
    assert_eq!(block_on(backend.send(message(&rig.user, &rig.user, "by token", "x").as_bytes(), &me, std::slice::from_ref(&me))), Err(MailError::Auth));

    backend.set_credential(Credential::OAuth2 { access_token: rig.user.clone() });
    block_on(backend.send(message(&rig.user, &rig.user, "by token", "x").as_bytes(), &me, std::slice::from_ref(&me))).unwrap();
    assert_eq!(full_sync(&backend, &inbox()).added.len(), 2);
}

#[test]
fn smtp_refusals_are_mapped() {
    let Some(rig) = Rig::new("smtp") else { return };
    let me = Address::new("Me", &rig.user);
    // No recipients: refused before connecting.
    assert!(matches!(block_on(rig.backend.send(b"Subject: x\r\n\r\nx\r\n", &me, &[])), Err(MailError::Server(_))));
    // A wrong SMTP password is an auth error, like IMAP's.
    let mut wrong = config(&rig.user);
    wrong.credential = Credential::Password("not the password".into());
    let backend = ImapBackend::new(wrong).unwrap();
    assert_eq!(block_on(backend.send(b"Subject: x\r\n\r\nx\r\n", &me, std::slice::from_ref(&me))), Err(MailError::Auth));
    // Nothing listening: a network error.
    let mut nowhere = config(&rig.user);
    nowhere.smtp = Endpoint::new(HOST, 1, Security::Plain);
    let backend = ImapBackend::new(nowhere).unwrap();
    assert!(matches!(block_on(backend.send(b"Subject: x\r\n\r\nx\r\n", &me, std::slice::from_ref(&me))), Err(MailError::Network(_))));
}

#[test]
fn the_backend_works_inside_someone_elses_runtime() {
    if !available() { return; }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let user = unique_user("shared");
    deliver(&user, &message("seed@localhost", &user, "seed", "x"));
    let backend = ImapBackend::with_handle(config(&user), runtime.handle().clone()).unwrap();
    // Awaited on the host's runtime, and dropped inside it without a panic.
    let count = runtime.block_on(async move { let n = backend.list_folders().await.unwrap().len(); drop(backend); n });
    assert_eq!(count, 1);
    // An owned-runtime backend can be dropped from async code too.
    let owned = ImapBackend::new(config(&user)).unwrap();
    runtime.block_on(async move { assert_eq!(owned.list_folders().await.unwrap().len(), 1); drop(owned); });
}

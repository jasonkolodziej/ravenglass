//! `ImapBackend` against Stalwart, a second, independent IMAP and SMTP implementation (IMAP4rev2,
//! CONDSTORE/QRESYNC, SPECIAL-USE with Microsoft-style names, submission with STARTTLS and with
//! implicit TLS). Skipped unless `RAVENGLASS_STALWART_TEST=1`:
//!
//! ```text
//! tools/stalwart.sh start           # or let the tests start it
//! RAVENGLASS_STALWART_TEST=1 cargo test -p mail-imap --test stalwart
//! ```

mod real;

use mail_core::*;
use mail_imap::{Credential, Endpoint, ImapBackend, Security};
use ravenglass_engine::runner::block_on;
use real::*;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

fn full_sync(backend: &ImapBackend, folder: &FolderId) -> FolderDelta { block_on(backend.sync_folder(folder, &SyncToken::default())).unwrap() }

fn find<'a>(delta: &'a FolderDelta, subject: &str) -> &'a Envelope {
    delta.added.iter().find(|e| e.subject == subject).unwrap_or_else(|| panic!("no {subject:?} among {:?}", delta.added.iter().map(|e| &e.subject).collect::<Vec<_>>()))
}

/// Wait until a full sync of `folder` has `count` messages (delivery is asynchronous on Stalwart).
fn settled(backend: &ImapBackend, folder: &FolderId, count: usize) -> FolderDelta {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let delta = full_sync(backend, folder);
        if delta.added.len() >= count || Instant::now() > deadline { return delta; }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn folders_come_with_their_special_use_roles() {
    let Some(server) = stalwart() else { return };
    let user = server.user("folders");
    server.deliver(&user, "seed", "x");
    let backend = ImapBackend::new(server.config(&user)).unwrap();
    let folders = block_on(backend.list_folders()).unwrap();
    let role = |role| folders.iter().find(|f| f.role == role).map(|f| f.name.clone());
    assert_eq!(role(FolderRole::Inbox).as_deref(), Some("Inbox"));
    assert_eq!(role(FolderRole::Sent).as_deref(), Some("Sent Items"));
    assert_eq!(role(FolderRole::Trash).as_deref(), Some("Deleted Items"));
    assert_eq!(role(FolderRole::Junk).as_deref(), Some("Junk Mail"));
    assert_eq!(role(FolderRole::Drafts).as_deref(), Some("Drafts"));
    let inbox_folder = folders.iter().find(|f| f.role == FolderRole::Inbox).unwrap();
    assert_eq!((inbox_folder.total, inbox_folder.unread), (1, 1));
}

#[test]
fn sync_flags_move_and_draft() {
    let Some(server) = stalwart() else { return };
    let user = server.user("ops");
    server.imap(&user, &["CREATE Archive"]);
    for subject in ["one", "two", "three"] { server.deliver(&user, subject, &format!("Body of {subject}.")); }
    let backend = ImapBackend::new(server.config(&user)).unwrap();
    let first = full_sync(&backend, &inbox());
    assert_eq!(first.added.len(), 3);
    assert_eq!(find(&first, "two").preview, "Body of two.");
    let (one, two, three) = (find(&first, "one").id.clone(), find(&first, "two").id.clone(), find(&first, "three").id.clone());

    let quiet = block_on(backend.sync_folder(&inbox(), &first.token)).unwrap();
    assert_eq!((quiet.added.len(), quiet.changed_flags.len(), quiet.removed.len()), (0, 0, 0));

    // Flags from this backend and from another client both come back, and only those.
    block_on(backend.set_flags(std::slice::from_ref(&one), Flags::SEEN | Flags::FLAGGED, Flags::empty())).unwrap();
    server.imap(&user, &["SELECT INBOX", "UID STORE 2 +FLAGS (\\Answered)"]);
    let flagged = block_on(ImapBackend::new(server.config(&user)).unwrap().sync_folder(&inbox(), &quiet.token)).unwrap();
    let mut changed = flagged.changed_flags.clone();
    changed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut expected = vec![(one.clone(), Flags::SEEN | Flags::FLAGGED), (two.clone(), Flags::ANSWERED)];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(changed, expected);
    block_on(backend.set_flags(std::slice::from_ref(&one), Flags::empty(), Flags::FLAGGED)).unwrap();
    let unflagged = block_on(backend.sync_folder(&inbox(), &flagged.token)).unwrap();
    assert_eq!(unflagged.changed_flags, vec![(one.clone(), Flags::SEEN)]);

    // Move to Archive and back.
    block_on(backend.move_to(std::slice::from_ref(&three), &folder("Archive"))).unwrap();
    let after = block_on(backend.sync_folder(&inbox(), &unflagged.token)).unwrap();
    assert_eq!(after.removed, vec![three.clone()]);
    let archive = full_sync(&backend, &folder("Archive"));
    assert_eq!(archive.added.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["three"]);
    block_on(backend.move_to(std::slice::from_ref(&archive.added[0].id), &inbox())).unwrap();
    assert_eq!(block_on(backend.sync_folder(&inbox(), &after.token)).unwrap().added.len(), 1);

    // A draft lands in Drafts with its flags, byte for byte.
    let raw = message(&user.address, "x@example.org", "unfinished", "Dear someone,");
    let draft = block_on(backend.append_draft(raw.as_bytes())).unwrap();
    assert!(draft.0.starts_with(&format!("{ACCOUNT}/Drafts;")), "{draft:?}");
    assert_eq!(block_on(backend.fetch_body(&draft)).unwrap().bytes, raw.as_bytes());
    let drafts = full_sync(&backend, &folder("Drafts"));
    assert_eq!(drafts.added[0].id, draft);
    assert!(drafts.added[0].flags.contains(Flags::DRAFT | Flags::SEEN));
}

#[test]
fn previews_and_attachments_from_stalwart() {
    let Some(server) = stalwart() else { return };
    let user = server.user("preview");
    let raw = format!("From: A <a@example.org>\r\nTo: {}\r\nSubject: with file\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=frontier\r\n\r\n--frontier\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>See the <b>attached</b> route.</p>{}</body></html>\r\n--frontier\r\nContent-Type: application/pdf; name=\"route-north.pdf\"\r\nContent-Disposition: attachment; filename=\"route-north.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQKJcfsj6IKNSAwIG9iago8PC9MZW5ndGggNiAwIFI+PgpzdHJlYW0K\r\n--frontier--\r\n", user.address, "<p>filler</p>".repeat(500));
    server.deliver_to(&user, "INBOX", &raw);
    let backend = ImapBackend::new(server.config(&user)).unwrap();
    let delta = full_sync(&backend, &inbox());
    let envelope = find(&delta, "with file");
    assert_eq!(envelope.preview.split(" filler").next(), Some("See the attached route."));
    assert_eq!(envelope.attachments.iter().map(|a| (a.name.as_str(), a.mime.as_str())).collect::<Vec<_>>(), [("route-north.pdf", "application/pdf")]);
    assert_eq!(block_on(backend.fetch_body(&envelope.id)).unwrap().bytes, raw.as_bytes());
}

fn next_event(received: &Receiver<IdleEvent>, wanted: &IdleEvent) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Ok(event) = received.recv_timeout(Duration::from_millis(200)) {
            if &event == wanted { return; }
            seen.push(event);
        }
    }
    panic!("no {wanted:?} within 20 s; saw {seen:?}");
}

#[test]
fn idle_reports_new_mail_flag_changes_and_expunges() {
    let Some(server) = stalwart() else { return };
    let user = server.user("idle");
    server.deliver(&user, "one", "x");
    server.deliver(&user, "two", "x");
    let backend = Arc::new(ImapBackend::new(server.config(&user)).unwrap());
    let (events, received) = channel();
    let watcher = { let backend = backend.clone(); std::thread::spawn(move || block_on(backend.idle(&inbox(), events))) };
    std::thread::sleep(Duration::from_millis(1000));

    server.deliver(&user, "three", "x");
    next_event(&received, &IdleEvent::Exists(inbox()));
    server.imap(&user, &["SELECT INBOX", "UID STORE 1 +FLAGS (\\Seen)"]);
    next_event(&received, &IdleEvent::FlagsChanged(inbox()));
    server.imap(&user, &["SELECT INBOX", "UID STORE 2 +FLAGS (\\Deleted)", "EXPUNGE"]);
    next_event(&received, &IdleEvent::Expunged(inbox()));
    // The command connection works while the watch is on.
    assert_eq!(full_sync(&backend, &inbox()).added.len(), 2);

    drop(received);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !watcher.is_finished() {
        assert!(Instant::now() < deadline, "the watch did not stop after its receiver was dropped");
        server.deliver(&user, "anyone there?", "x");
        std::thread::sleep(Duration::from_millis(500));
    }
    assert_eq!(watcher.join().unwrap(), Ok(()));
}

/// Send to ourselves through `smtp` and find the message in the inbox and its copy in Sent.
fn submits(port: u16, security: Security) {
    let Some(server) = stalwart() else { return };
    let user = server.user(&format!("smtp{port}"));
    let mut config = server.config(&user);
    config.smtp = Endpoint::new("localhost", port, security);
    let backend = ImapBackend::new(config).unwrap();
    let me = Address::new("Me", &user.address);
    let subject = format!("over {security:?}");
    let raw = format!("From: Me <{0}>\r\nTo: Me <{0}>\r\nBcc: hidden@example.org\r\nSubject: {subject}\r\nDate: Mon, 23 Nov 2026 12:30:00 +0000\r\nMessage-ID: <submit-{port}@test>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello there.\r\n.a line that starts with a dot\r\n", user.address);
    block_on(backend.send(raw.as_bytes(), &me, std::slice::from_ref(&me))).unwrap();

    let delta = settled(&backend, &inbox(), 1);
    let got = find(&delta, &subject);
    assert_eq!(got.from, me);
    let delivered = String::from_utf8(block_on(backend.fetch_body(&got.id)).unwrap().bytes).unwrap();
    assert!(!delivered.contains("hidden@example.org") && delivered.contains("\r\n.a line that starts with a dot\r\n"));
    let sent = block_on(backend.list_folders()).unwrap().into_iter().find(|f| f.role == FolderRole::Sent).unwrap();
    let copy = full_sync(&backend, &sent.id);
    assert_eq!(copy.added.len(), 1);
    assert_eq!(block_on(backend.fetch_body(&copy.added[0].id)).unwrap().bytes, raw.as_bytes());

    // A wrong password is an auth error on submission too; an untrusted certificate is a TLS one.
    let mut wrong = server.config(&user);
    wrong.smtp = Endpoint::new("localhost", port, security);
    wrong.credential = Credential::Password("not the password".into());
    assert_eq!(block_on(ImapBackend::new(wrong).unwrap().send(raw.as_bytes(), &me, std::slice::from_ref(&me))), Err(MailError::Auth));
    let mut untrusted = server.config(&user);
    untrusted.smtp = Endpoint::new("localhost", port, security);
    untrusted.extra_roots.clear();
    let refused = block_on(ImapBackend::new(untrusted).unwrap().send(raw.as_bytes(), &me, std::slice::from_ref(&me))).unwrap_err();
    assert!(matches!(&refused, MailError::Network(text) if text.starts_with("TLS:")), "{refused:?}");
}

#[test]
fn submission_with_starttls() { if let Some(server) = stalwart() { submits(server.submission.unwrap().0, Security::StartTls) } }

#[test]
fn submission_with_implicit_tls() { if let Some(server) = stalwart() { submits(server.submission.unwrap().1, Security::Tls) } }

#[test]
fn implicit_tls_imap_and_a_wrong_password() {
    let Some(server) = stalwart() else { return };
    let user = server.user("imaps");
    let mut config = server.config(&user);
    config.imap = Endpoint::new("localhost", server.imaps, Security::Tls);
    assert!(block_on(ImapBackend::new(config).unwrap().list_folders()).is_ok());
    let mut wrong = server.config(&user);
    wrong.credential = Credential::Password("not the password".into());
    assert_eq!(block_on(ImapBackend::new(wrong).unwrap().list_folders()), Err(MailError::Auth));
}

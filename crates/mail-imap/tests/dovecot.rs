//! `ImapBackend` against Dovecot, the most widely deployed IMAP server: STARTTLS, CONDSTORE,
//! SPECIAL-USE, an `INBOX.` namespace, modified UTF-7 names, partial fetches, IDLE and a large
//! mailbox, all of which GreenMail lacks. Skipped unless `RAVENGLASS_DOVECOT_TEST=1`:
//!
//! ```text
//! apt-get install dovecot-imapd     # once
//! tools/dovecot.sh start            # or let the tests start it
//! RAVENGLASS_DOVECOT_TEST=1 cargo test -p mail-imap --test dovecot -- --nocapture
//! ```

mod real;

use mail_core::*;
use mail_imap::{Credential, Endpoint, ImapBackend, ImapConfig, Security};
use ravenglass_engine::runner::block_on;
use real::*;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

fn full_sync(backend: &ImapBackend, folder: &FolderId) -> FolderDelta { block_on(backend.sync_folder(folder, &SyncToken::default())).unwrap() }

fn find<'a>(delta: &'a FolderDelta, subject: &str) -> &'a Envelope {
    delta.added.iter().find(|e| e.subject == subject).unwrap_or_else(|| panic!("no {subject:?} among {:?}", delta.added.iter().map(|e| &e.subject).collect::<Vec<_>>()))
}

/// A plain connection through a recorder (Dovecot accepts a login without TLS on loopback).
fn recorded(server: &Server, user: &User) -> (ImapBackend, Recorder) {
    let recorder = Recorder::start(server.imap_starttls);
    let mut config = server.config(user);
    config.imap = Endpoint::new(HOST, recorder.port, Security::Plain);
    (ImapBackend::new(config).unwrap(), recorder)
}

#[test]
fn starttls_and_implicit_tls_connect_and_an_untrusted_certificate_is_refused() {
    let Some(server) = dovecot() else { return };
    let user = server.user("tls");
    server.deliver(&user, "over tls", "Encrypted.");

    // STARTTLS (port 3144): the upgrade completes before the login.
    let backend = ImapBackend::new(server.config(&user)).unwrap();
    let folders = block_on(backend.list_folders()).unwrap();
    assert_eq!(folders.iter().find(|f| f.role == FolderRole::Inbox).map(|f| f.total), Some(1));
    assert_eq!(full_sync(&backend, &inbox()).added[0].preview, "Encrypted.");

    // Implicit TLS (port 3994).
    let mut tls = server.config(&user);
    tls.imap = Endpoint::new("localhost", server.imaps, Security::Tls);
    assert_eq!(block_on(ImapBackend::new(tls).unwrap().list_folders()).unwrap().len(), folders.len());

    // Without the certificate in the trust store, both refuse before any credential is sent.
    for (port, security) in [(server.imap_starttls, Security::StartTls), (server.imaps, Security::Tls)] {
        let mut untrusted = server.config(&user);
        untrusted.imap = Endpoint::new("localhost", port, security);
        untrusted.extra_roots.clear();
        let refused = block_on(ImapBackend::new(untrusted).unwrap().list_folders()).unwrap_err();
        assert!(matches!(&refused, MailError::Network(text) if text.starts_with("TLS:")), "{security:?}: {refused:?}");
    }
    // A wrong password over TLS is an auth error, not a TLS one.
    let mut wrong = server.config(&user);
    wrong.credential = Credential::Password("not the password".into());
    assert_eq!(block_on(ImapBackend::new(wrong).unwrap().list_folders()), Err(MailError::Auth));
}

#[test]
fn condstore_reports_only_what_changed_and_skips_an_unchanged_folder() {
    let Some(server) = dovecot() else { return };
    let user = server.user("condstore");
    for subject in ["one", "two", "three"] { server.deliver(&user, subject, "x"); }
    let (backend, recorder) = recorded(server, &user);
    let first = full_sync(&backend, &inbox());
    assert_eq!(first.added.len(), 3);
    let (one, two, three) = (find(&first, "one").id.clone(), find(&first, "two").id.clone(), find(&first, "three").id.clone());
    recorder.take();

    // Nothing happened: one SELECT, and no FETCH or SEARCH at all.
    let quiet = block_on(backend.sync_folder(&inbox(), &first.token)).unwrap();
    assert_eq!((quiet.added.len(), quiet.changed_flags.len(), quiet.removed.len()), (0, 0, 0));
    let commands = recorder.take();
    assert_eq!(commands.iter().filter(|c| c.starts_with("SELECT")).count(), 1, "{commands:?}");
    assert!(!commands.iter().any(|c| c.contains("FETCH") || c.contains("SEARCH")), "the unchanged folder was fetched: {commands:?}");

    // Another client flags one message: CONDSTORE returns that one and nothing else.
    server.imap(&user, &["SELECT INBOX", "UID STORE 2 +FLAGS (\\Flagged)"]);
    let flagged = block_on(backend.sync_folder(&inbox(), &quiet.token)).unwrap();
    assert_eq!(flagged.changed_flags, vec![(two.clone(), Flags::FLAGGED)]);
    assert!(flagged.added.is_empty() && flagged.removed.is_empty());
    assert!(recorder.take().iter().any(|c| c.contains("CHANGEDSINCE")), "the flags came from a CHANGEDSINCE fetch");

    // A backend that has just started has no flag cache; with CONDSTORE it still reports only
    // the change (GreenMail, without CONDSTORE, reports every known message).
    server.imap(&user, &["SELECT INBOX", "UID STORE 1 +FLAGS (\\Seen)"]);
    let restarted = ImapBackend::new(server.config(&user)).unwrap();
    let seen = block_on(restarted.sync_folder(&inbox(), &flagged.token)).unwrap();
    assert_eq!(seen.changed_flags, vec![(one, Flags::SEEN)]);

    // An expunge and a delivery together.
    server.imap(&user, &["SELECT INBOX", "UID STORE 3 +FLAGS (\\Deleted)", "EXPUNGE"]);
    server.deliver(&user, "four", "x");
    let mixed = block_on(restarted.sync_folder(&inbox(), &seen.token)).unwrap();
    assert_eq!(mixed.removed, vec![three]);
    assert_eq!(mixed.added.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["four"]);
    // The \Deleted flag set just before the expunge is not reported for a message that is gone.
    assert!(mixed.changed_flags.is_empty(), "{:?}", mixed.changed_flags);
}

#[test]
fn special_use_attributes_give_the_roles() {
    let Some(server) = dovecot() else { return };
    let user = server.user("specialuse");
    // Dovecot marks Sent, Junk, Archive, Drafts and Trash with SPECIAL-USE flags; the look-alikes have none.
    server.imap(&user, &["CREATE \"Sent Items\"", "CREATE Sent", "CREATE Spam", "CREATE Junk", "CREATE Archive", "CREATE Drafts", "CREATE Trash", "CREATE \"Deleted Messages\""]);
    let listed = server.imap(&user, &["LIST \"\" \"*\""]);
    assert!(listed.iter().any(|l| l.contains("\\Sent") && l.ends_with("Sent")), "{listed:?}");
    let backend = ImapBackend::new(server.config(&user)).unwrap();
    let folders = block_on(backend.list_folders()).unwrap();
    let role = |mailbox: &str| folders.iter().find(|f| f.id == folder(mailbox)).unwrap_or_else(|| panic!("{mailbox}: {folders:?}")).role;
    assert_eq!(role("INBOX"), FolderRole::Inbox);
    assert_eq!(role("Sent"), FolderRole::Sent);
    assert_eq!(role("Sent Items"), FolderRole::Other, "the server's attribute beats a matching name");
    assert_eq!(role("Junk"), FolderRole::Junk);
    assert_eq!(role("Spam"), FolderRole::Other);
    assert_eq!(role("Archive"), FolderRole::Archive);
    assert_eq!(role("Drafts"), FolderRole::Drafts);
    assert_eq!(role("Trash"), FolderRole::Trash);
    assert_eq!(role("Deleted Messages"), FolderRole::Other);

    // A draft goes to the attributed Drafts folder.
    let id = block_on(backend.append_draft(message(&user.address, "x@example.org", "draft", "…").as_bytes())).unwrap();
    assert!(id.0.starts_with(&format!("{}/Drafts;", ACCOUNT)), "{id:?}");
}

#[test]
fn an_inbox_namespace_keeps_new_folders_under_inbox() {
    let Some(server) = dovecot() else { return };
    // Courier-style layout: everything under `INBOX.`, `.` separator, and only an inbox so far.
    let user = server.user_with("dotted", true);
    server.deliver(&user, "seed", "x");
    let sink = SmtpSink::start();
    let mut config = server.config(&user);
    config.smtp = Endpoint::new(HOST, sink.port, Security::Plain);
    let backend = ImapBackend::new(config).unwrap();
    let folders = block_on(backend.list_folders()).unwrap();
    assert_eq!(folders.iter().map(|f| f.id.0.as_str()).collect::<Vec<_>>(), [inbox().0.as_str()]);

    // No Drafts yet: the server refuses a top-level `Drafts` ("nonexistent namespace"), so it is
    // created as INBOX.Drafts, where Dovecot gives it the \Drafts attribute.
    let raw = message(&user.address, "x@example.org", "unfinished", "Dear someone,");
    let draft = block_on(backend.append_draft(raw.as_bytes())).unwrap();
    assert!(draft.0.starts_with(&format!("{ACCOUNT}/INBOX.Drafts;")), "{draft:?}");
    assert_eq!(block_on(backend.fetch_body(&draft)).unwrap().bytes, raw.as_bytes());

    // Sending saves the copy to INBOX.Sent the same way.
    let me = Address::new("Me", &user.address);
    block_on(backend.send(message(&user.address, &user.address, "sent", "Out it goes.").as_bytes(), &me, std::slice::from_ref(&me))).unwrap();
    assert_eq!(sink.received.lock().unwrap().len(), 1);
    let folders = block_on(backend.list_folders()).unwrap();
    let by_role = |role| folders.iter().find(|f| f.role == role).unwrap_or_else(|| panic!("{role:?}: {folders:?}"));
    assert_eq!(by_role(FolderRole::Drafts).id, folder("INBOX.Drafts"));
    let sent = by_role(FolderRole::Sent);
    assert_eq!((sent.id.clone(), sent.name.as_str(), sent.total), (folder("INBOX.Sent"), "Sent", 1));
    assert_eq!(sent.path, ["INBOX", "Sent"]);
    let listed = server.imap(&user, &["LIST \"\" \"*\""]);
    assert!(listed.iter().any(|l| l.contains("\\Sent") && l.ends_with("INBOX.Sent")), "{listed:?}");
}

#[test]
fn modified_utf7_folder_names_round_trip() {
    let Some(server) = dovecot() else { return };
    let user = server.user("utf7");
    server.deliver(&user, "traveller", "x");
    // Entwürfe, 日本語, a literal ampersand, and a nested name with a space.
    let names = [("Entw&APw-rfe", "Entwürfe"), ("&ZeVnLIqe-", "日本語"), ("Q&-A", "Q&A"), ("Reise/K&APY-ln 2026", "Köln 2026")];
    let creates: Vec<String> = names.iter().map(|(raw, _)| format!("CREATE {}", quoted(raw))).collect();
    server.imap(&user, &creates.iter().map(String::as_str).collect::<Vec<_>>());

    let backend = ImapBackend::new(server.config(&user)).unwrap();
    let folders = block_on(backend.list_folders()).unwrap();
    for (raw, shown) in names {
        let found = folders.iter().find(|f| f.id == folder(raw)).unwrap_or_else(|| panic!("{raw} not listed: {folders:?}"));
        assert_eq!(found.name, shown);
    }
    assert_eq!(folders.iter().find(|f| f.id == folder("Reise/K&APY-ln 2026")).unwrap().path, ["Reise", "Köln 2026"]);

    // The ids work for every operation: a message travels through each folder and back.
    let mut current = find(&full_sync(&backend, &inbox()), "traveller").id.clone();
    let mut from = inbox();
    for (raw, _) in names {
        block_on(backend.move_to(std::slice::from_ref(&current), &folder(raw))).unwrap();
        let there = full_sync(&backend, &folder(raw));
        assert_eq!(there.added.len(), 1, "{raw}");
        assert_eq!(block_on(backend.sync_folder(&from, &SyncToken::default())).unwrap().added.len(), 0);
        current = there.added[0].id.clone();
        from = folder(raw);
    }
    block_on(backend.set_flags(std::slice::from_ref(&current), Flags::FLAGGED, Flags::empty())).unwrap();
    assert!(block_on(backend.fetch_body(&current)).unwrap().bytes.ends_with(b"x\r\n"));
    let status = server.imap(&user, &["STATUS \"Reise/K&APY-ln 2026\" (MESSAGES)"]);
    assert!(status.iter().any(|l| l.contains("MESSAGES 1")), "{status:?}");
}

#[test]
fn previews_come_from_the_partial_fetch() {
    let Some(server) = dovecot() else { return };
    let user = server.user("preview");
    let long = "A fairly long line of body text. ".repeat(300);
    let html = format!("<html><head><style>p {{ color: red }}</style></head><body><p>Hello <b>there</b>, this is the HTML part.</p>{}</body></html>", "<p>filler</p>".repeat(400));
    let qp = "Gr=C3=BC=C3=9Fe aus K=C3=B6ln, mit einer sehr langen Zeile die weich umbrochen =\r\nwird.";
    let b64 = "VGhpcyBib2R5IGlzIGJhc2U2NCBlbmNvZGVkLg==";
    let raws = [
        ("plain long", format!("Content-Type: text/plain; charset=utf-8\r\n\r\n{long}\r\n")),
        ("alternative", format!("Content-Type: multipart/alternative; boundary=\"b1\"\r\n\r\nPreamble nobody reads.\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe plain alternative.\r\n--b1\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n--b1--\r\n")),
        ("html only", format!("Content-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n")),
        ("quoted printable", format!("Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{qp}\r\n")),
        ("base64", format!("Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{b64}\r\n")),
        ("mixed", format!("Content-Type: multipart/mixed; boundary=\"m\"\r\n\r\n--m\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n--m\r\nContent-Type: application/pdf; name=\"a.pdf\"\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--m--\r\n", "JVBERi0xLjQK".repeat(2000))),
    ];
    for (subject, rest) in &raws {
        server.deliver_to(&user, "INBOX", &format!("From: a@example.org\r\nTo: {}\r\nSubject: {subject}\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\n{rest}", user.address));
    }
    let (backend, recorder) = recorded(server, &user);
    let delta = full_sync(&backend, &inbox());
    let commands = recorder.take();
    // One FETCH with the partial body; none of the fallbacks for servers that answer it badly.
    let fetches: Vec<&String> = commands.iter().filter(|c| c.contains("FETCH")).collect();
    assert_eq!(fetches.len(), 1, "{commands:?}");
    assert!(fetches[0].contains("BODY.PEEK[TEXT]<0.3072>") && fetches[0].contains("BODYSTRUCTURE"), "{fetches:?}");

    let preview = |subject: &str| find(&delta, subject).preview.clone();
    assert_eq!(preview("plain long").chars().count(), 200);
    assert!(preview("plain long").starts_with("A fairly long line of body text. A fairly"));
    assert_eq!(preview("alternative"), "The plain alternative.");
    assert!(preview("html only").starts_with("Hello there, this is the HTML part."), "{:?}", preview("html only"));
    assert!(!preview("html only").contains("color"), "no CSS in the preview");
    assert_eq!(preview("quoted printable"), "Grüße aus Köln, mit einer sehr langen Zeile die weich umbrochen wird.");
    assert_eq!(preview("base64"), "This body is base64 encoded.");
    assert_eq!(preview("mixed"), "See attached.");
    assert_eq!(find(&delta, "mixed").attachments.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["a.pdf"]);

    // The next sync still asks for the partial body: the server's answer was never refused.
    server.deliver(&user, "later", "Arrived later.");
    let again = block_on(backend.sync_folder(&inbox(), &delta.token)).unwrap();
    assert_eq!(find(&again, "later").preview, "Arrived later.");
    assert!(recorder.take().iter().any(|c| c.contains("BODY.PEEK[TEXT]<0.3072>")));
}

fn next_event(received: &Receiver<IdleEvent>, wanted: &IdleEvent) -> Vec<IdleEvent> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Ok(event) = received.recv_timeout(Duration::from_millis(200)) {
            seen.push(event.clone());
            if &event == wanted { return seen; }
        }
    }
    panic!("no {wanted:?} within 20 s; saw {seen:?}");
}

#[test]
fn idle_reports_new_mail_flag_changes_and_expunges() {
    let Some(server) = dovecot() else { return };
    let user = server.user("idle");
    server.deliver(&user, "one", "x");
    server.deliver(&user, "two", "x");
    let backend = Arc::new(ImapBackend::new(server.config(&user)).unwrap());
    let (events, received) = channel();
    let watcher = { let backend = backend.clone(); std::thread::spawn(move || block_on(backend.idle(&inbox(), events))) };
    std::thread::sleep(Duration::from_millis(1000));

    server.imap(&user, &["SELECT INBOX", "UID STORE 1 +FLAGS (\\Seen)"]);
    next_event(&received, &IdleEvent::FlagsChanged(inbox()));
    server.imap(&user, &["SELECT INBOX", "UID STORE 2 +FLAGS (\\Deleted)", "EXPUNGE"]);
    next_event(&received, &IdleEvent::Expunged(inbox()));
    server.deliver(&user, "three", "x");
    next_event(&received, &IdleEvent::Exists(inbox()));

    drop(received);
    server.deliver(&user, "four", "x");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !watcher.is_finished() {
        assert!(Instant::now() < deadline, "the watch did not stop after its receiver was dropped");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(watcher.join().unwrap(), Ok(()));
}

/// An mbox of `count` small messages, for `doveadm import`.
fn mbox(count: usize, to: &str) -> String {
    let mut out = String::with_capacity(count * 600);
    for i in 1..=count {
        out.push_str(&format!(
            "From sender@example.org Mon Nov 23 10:00:00 2026\nFrom: Sender {i} <sender{i}@example.org>\nTo: {to}\nSubject: Message {i}\nDate: Mon, 23 Nov 2026 10:00:00 +0000\nMessage-ID: <bulk-{i}@test>\nMIME-Version: 1.0\nContent-Type: text/plain; charset=utf-8\n\nBody of message {i}, with enough words to make a preview of a realistic length for the list.\nSecond line.\n\n"
        ));
    }
    out
}

#[test]
fn a_large_mailbox_syncs_in_one_pass_and_resyncs_quickly() {
    let Some(server) = dovecot() else { return };
    const COUNT: usize = 5000;
    let user = server.user("large");
    let source = std::env::temp_dir().join(format!("ravenglass-mbox-{}", user.login));
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("inbox"), mbox(COUNT, &user.address)).unwrap();
    // doveadm runs as the mail user, which must be able to read the file and write beside it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::set_permissions(source.join("inbox"), std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let location = format!("mbox:{0}:INBOX={0}/inbox:INDEX=MEMORY", source.display());
    let status = std::process::Command::new(tools_dir().join("dovecot.sh")).args(["doveadm", "import", "-u", &user.login, &location, "", "all"]).status().unwrap();
    std::fs::remove_dir_all(&source).unwrap();
    assert!(status.success(), "doveadm import failed");

    let backend = ImapBackend::new(server.config(&user)).unwrap();
    let started = Instant::now();
    let first = full_sync(&backend, &inbox());
    let first_time = started.elapsed();
    assert_eq!(first.added.len(), COUNT);
    let last = first.added.iter().find(|e| e.subject == format!("Message {COUNT}")).unwrap();
    assert!(last.preview.starts_with(&format!("Body of message {COUNT}, with enough words")), "{:?}", last.preview);
    assert!(first.token.0.len() < 64, "the token stays compact: {}", first.token.0);

    let started = Instant::now();
    let quiet = block_on(backend.sync_folder(&inbox(), &first.token)).unwrap();
    let quiet_time = started.elapsed();
    assert_eq!((quiet.added.len(), quiet.changed_flags.len(), quiet.removed.len()), (0, 0, 0));

    server.imap(&user, &["SELECT INBOX", "UID STORE 1234 +FLAGS (\\Seen)", "UID STORE 4321 +FLAGS (\\Deleted)", "EXPUNGE"]);
    server.deliver(&user, "newest", "x");
    let started = Instant::now();
    let changed = block_on(backend.sync_folder(&inbox(), &quiet.token)).unwrap();
    let changed_time = started.elapsed();
    assert_eq!((changed.added.len(), changed.changed_flags.len(), changed.removed.len()), (1, 1, 1));

    // A fresh backend (no connection, no flag cache) resuming from the token: reconnect + resync.
    let started = Instant::now();
    let resumed = block_on(ImapBackend::new(server.config(&user)).unwrap().sync_folder(&inbox(), &changed.token)).unwrap();
    let resumed_time = started.elapsed();
    assert_eq!((resumed.added.len(), resumed.changed_flags.len(), resumed.removed.len()), (0, 0, 0));
    eprintln!("dovecot, {COUNT} messages: first sync {first_time:.2?}, unchanged resync {quiet_time:.2?}, resync with 1 new + 1 flag + 1 expunge {changed_time:.2?}, cold resync {resumed_time:.2?}");
}

#[test]
fn the_engine_runs_over_dovecot() {
    let Some(server) = dovecot() else { return };
    use mail_core::machine::account_sync;
    use mail_store::Store;
    use ravenglass_engine::{Command, Config, Engine, Event};
    let user = server.user("engine");
    server.imap(&user, &["CREATE Archive", "CREATE Drafts"]);
    server.deliver(&user, "hello", "From the engine test.");
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config { undo_send_secs: 0.0 });
    let config: ImapConfig = server.config(&user);
    engine.add_backend(Arc::new(ImapBackend::new(config).unwrap()));
    let events = block_on(engine.handle(Command::OpenAccount(account()), 1000.0));
    assert!(events.contains(&Event::SyncState { account: account(), state: account_sync::State::Idle }), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, Event::Error { .. })), "{events:?}");
    let page = match block_on(engine.handle(Command::LoadRange { folder: inbox(), offset: 0, limit: 10 }, 1000.0)).remove(0) {
        Event::Envelopes { envelopes, .. } => envelopes,
        other => panic!("{other:?}"),
    };
    assert_eq!(page.iter().map(|e| e.preview.as_str()).collect::<Vec<_>>(), ["From the engine test."]);
    let moved = block_on(engine.handle(Command::MoveToRole { ids: vec![page[0].id.clone()], role: FolderRole::Archive }, 1001.0));
    assert!(moved.iter().any(|e| matches!(e, Event::Applied { sent: true, .. })), "{moved:?}");
    assert!(server.imap(&user, &["SELECT Archive", "UID SEARCH ALL"]).iter().any(|l| l == "* SEARCH 1"));
}

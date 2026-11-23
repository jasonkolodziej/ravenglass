//! `JmapBackend` over `HttpsTransport` against a real JMAP server (Stalwart): session discovery,
//! full and incremental sync, flags, moves, drafts, sending, and the engine end to end. Test mail
//! goes in over IMAP, so nothing here checks the backend against itself. Skipped unless
//! `RAVENGLASS_STALWART_TEST=1`:
//!
//! ```text
//! tools/stalwart.sh start           # or let the tests start it
//! RAVENGLASS_STALWART_TEST=1 cargo test -p mail-jmap --features http --test stalwart
//! ```

#[path = "../../mail-imap/tests/real/mod.rs"]
mod real;

use mail_core::*;
use mail_jmap::{Credential, HttpsTransport, JmapBackend};
use ravenglass_engine::runner::block_on;
use real::{Server, User, message, quoted, stalwart};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ACCOUNT: &str = "jm";

fn backend(server: &Server, user: &User) -> JmapBackend<HttpsTransport> { backend_as(server, user, ACCOUNT) }

fn backend_as(server: &Server, user: &User, account: &str) -> JmapBackend<HttpsTransport> {
    let credential = Credential::Basic { username: user.login.clone(), password: user.password.clone() };
    JmapBackend::new(AccountId::from(account), server.jmap.as_deref().unwrap(), HttpsTransport::with_roots(&credential, &server.roots()))
}

fn folders(backend: &JmapBackend<HttpsTransport>) -> Vec<Folder> { block_on(backend.list_folders()).unwrap() }

fn with_role(backend: &JmapBackend<HttpsTransport>, role: FolderRole) -> FolderId {
    folders(backend).into_iter().find(|f| f.role == role).unwrap_or_else(|| panic!("no {role:?} folder")).id
}

fn full_sync(backend: &JmapBackend<HttpsTransport>, folder: &FolderId) -> FolderDelta { block_on(backend.sync_folder(folder, &SyncToken::default())).unwrap() }

fn subjects(envelopes: &[Envelope]) -> Vec<String> {
    let mut list: Vec<String> = envelopes.iter().map(|e| e.subject.clone()).collect();
    list.sort();
    list
}

fn find<'a>(envelopes: &'a [Envelope], subject: &str) -> &'a Envelope {
    envelopes.iter().find(|e| e.subject == subject).unwrap_or_else(|| panic!("no {subject:?} among {:?}", subjects(envelopes)))
}

/// The IMAP flags of the message with `subject` in `mailbox`, as another client sees them.
fn imap_flags(server: &Server, user: &User, mailbox: &str, subject: &str) -> String {
    let lines = server.imap(user, &[&format!("SELECT {}", quoted(mailbox)), "FETCH 1:* (FLAGS BODY.PEEK[HEADER.FIELDS (SUBJECT)])"]);
    let at = lines.iter().position(|l| l.contains(&format!("Subject: {subject}"))).unwrap_or_else(|| panic!("{subject} not in {mailbox}: {lines:?}"));
    // The FLAGS come on the FETCH line just before the header literal.
    lines[..at].iter().rev().find(|l| l.contains("FETCH")).unwrap().clone()
}

#[test]
fn the_session_is_found_through_the_well_known_redirect_and_folders_have_roles() {
    let Some(server) = stalwart() else { return };
    let user = server.user("jsession");
    server.deliver(&user, "seed", "x");
    // `/.well-known/jmap` answers with a redirect; the credentials must survive it.
    assert!(server.jmap.as_deref().unwrap().ends_with("/.well-known/jmap"));
    let backend = backend(server, &user);
    let folders = folders(&backend);
    for role in [FolderRole::Inbox, FolderRole::Drafts, FolderRole::Sent, FolderRole::Trash, FolderRole::Junk] {
        assert!(folders.iter().any(|f| f.role == role), "{role:?}: {folders:?}");
    }
    let inbox = folders.iter().find(|f| f.role == FolderRole::Inbox).unwrap();
    assert_eq!((inbox.total, inbox.unread, inbox.account.0.as_str()), (1, 1, ACCOUNT));
}

#[test]
fn a_wrong_password_and_an_untrusted_certificate_are_refused() {
    let Some(server) = stalwart() else { return };
    let user = server.user("jauth");
    let url = server.jmap.as_deref().unwrap();
    let wrong = Credential::Basic { username: user.login.clone(), password: "not the password".into() };
    let backend = JmapBackend::new(AccountId::from(ACCOUNT), url, HttpsTransport::with_roots(&wrong, &server.roots()));
    assert_eq!(block_on(backend.list_folders()), Err(MailError::Auth));
    let right = Credential::Basic { username: user.login.clone(), password: user.password.clone() };
    let untrusted = JmapBackend::new(AccountId::from(ACCOUNT), url, HttpsTransport::new(&right));
    assert!(matches!(block_on(untrusted.list_folders()), Err(MailError::Network(_))));
}

#[test]
fn full_and_incremental_sync_follow_another_client() {
    let Some(server) = stalwart() else { return };
    let user = server.user("jsync");
    for subject in ["one", "two", "three"] { server.deliver(&user, subject, &format!("Body of {subject}.")); }
    let backend = backend(server, &user);
    let inbox = with_role(&backend, FolderRole::Inbox);
    let first = full_sync(&backend, &inbox);
    assert_eq!(subjects(&first.added), ["one", "three", "two"]);
    let two = find(&first.added, "two");
    assert_eq!(two.preview, "Body of two.");
    assert_eq!(two.from, Address::new("Sender", "sender@example.org"));
    assert_eq!(two.to, vec![Address { name: None, email: user.address.clone() }]);
    assert_eq!(two.date, 1_795_428_000, "receivedAt of an APPEND without a date is the Date header on Stalwart");
    assert!(two.is_unread() && !two.thread.0.is_empty());

    // Nothing happened: nothing comes back.
    let quiet = block_on(backend.sync_folder(&inbox, &first.token)).unwrap();
    assert_eq!((quiet.added.len(), quiet.changed_flags.len(), quiet.removed.len()), (0, 0, 0), "{quiet:?}");

    // Another client reads one, deletes one, and new mail arrives.
    server.imap(&user, &["SELECT INBOX", "STORE 1 +FLAGS (\\Seen)", "STORE 2 +FLAGS (\\Deleted)", "EXPUNGE"]);
    server.deliver(&user, "four", "Later.");
    let second = block_on(backend.sync_folder(&inbox, &quiet.token)).unwrap();
    let mut seen_now: Vec<(String, bool)> = second.added.iter().map(|e| (e.subject.clone(), e.flags.contains(Flags::SEEN))).collect();
    seen_now.sort();
    // Changed messages come back whole (the store upserts them); a destroyed one is removed.
    assert_eq!(seen_now, [("four".to_owned(), false), ("one".to_owned(), true)]);
    assert_eq!(second.removed, vec![find(&first.added, "two").id.clone()]);
    assert_ne!(second.token, quiet.token);

    // Mail delivered to another folder does not show up here, and is not reported as removed
    // from here either.
    server.imap(&user, &["CREATE Elsewhere"]);
    server.deliver_to(&user, "Elsewhere", &message("a@example.org", &user.address, "elsewhere", "x"));
    let third = block_on(backend.sync_folder(&inbox, &second.token)).unwrap();
    assert!(third.added.is_empty() && third.removed.is_empty(), "{third:?}");
}

#[test]
fn paging_covers_more_than_one_page() {
    let Some(server) = stalwart() else { return };
    const COUNT: usize = 300;
    let user = server.user("jpages");
    let appends: Vec<String> = (1..=COUNT).map(|i| {
        let raw = message("a@example.org", &user.address, &format!("bulk {i}"), "x");
        format!("APPEND INBOX {{{}+}}\r\n{raw}", raw.len())
    }).collect();
    server.imap(&user, &appends.iter().map(String::as_str).collect::<Vec<_>>());
    let backend = backend(server, &user);
    let inbox = with_role(&backend, FolderRole::Inbox);
    let started = Instant::now();
    let first = full_sync(&backend, &inbox);
    let full_time = started.elapsed();
    assert_eq!(first.added.len(), COUNT);
    // Every message changes: more changes than one Email/changes page holds.
    server.imap(&user, &["SELECT INBOX", "STORE 1:* +FLAGS (\\Flagged)"]);
    let started = Instant::now();
    let changed = block_on(backend.sync_folder(&inbox, &first.token)).unwrap();
    let changes_time = started.elapsed();
    assert_eq!(changed.added.len(), COUNT);
    assert!(changed.added.iter().all(|e| e.flags.contains(Flags::FLAGGED)));
    eprintln!("stalwart JMAP, {COUNT} messages: full sync {full_time:.2?}, Email/changes for all {changes_time:.2?}");
}

#[test]
fn flags_moves_and_drafts_reach_the_server() {
    let Some(server) = stalwart() else { return };
    let user = server.user("jops");
    server.imap(&user, &["CREATE Archive"]);
    server.deliver(&user, "mover", "x");
    let backend = backend(server, &user);
    let inbox = with_role(&backend, FolderRole::Inbox);
    let first = full_sync(&backend, &inbox);
    let mover = first.added[0].id.clone();

    block_on(backend.set_flags(std::slice::from_ref(&mover), Flags::SEEN | Flags::FLAGGED | Flags::ANSWERED, Flags::empty())).unwrap();
    let flags = imap_flags(server, &user, "INBOX", "mover");
    assert!(flags.contains("\\Seen") && flags.contains("\\Flagged") && flags.contains("\\Answered"), "{flags}");
    // The engine restores flags with `add = before, remove = !before`, unnamed bits included.
    block_on(backend.set_flags(std::slice::from_ref(&mover), Flags::SEEN, Flags(!Flags::SEEN.0))).unwrap();
    let flags = imap_flags(server, &user, "INBOX", "mover");
    assert!(flags.contains("\\Seen") && !flags.contains("\\Flagged") && !flags.contains("\\Answered"), "{flags}");

    let archive = folders(&backend).into_iter().find(|f| f.name == "Archive").unwrap().id;
    block_on(backend.move_to(std::slice::from_ref(&mover), &archive)).unwrap();
    let after = block_on(backend.sync_folder(&inbox, &first.token)).unwrap();
    // A moved message keeps its JMAP id: the inbox does not remove it, the archive's sync moves it.
    assert!(after.removed.is_empty() && after.added.is_empty(), "{after:?}");
    let there = full_sync(&backend, &archive);
    // A JMAP id survives a move.
    assert_eq!(there.added.iter().map(|e| &e.id).collect::<Vec<_>>(), [&mover]);
    assert!(server.imap(&user, &["SELECT Archive", "SEARCH ALL"]).iter().any(|l| l == "* SEARCH 1"));

    // A draft is uploaded and imported into Drafts, byte for byte.
    let raw = message(&user.address, "x@example.org", "unfinished", "Dear someone,");
    let draft = block_on(backend.append_draft(raw.as_bytes())).unwrap();
    assert_eq!(block_on(backend.fetch_body(&draft)).unwrap().bytes, raw.as_bytes());
    let drafts = full_sync(&backend, &with_role(&backend, FolderRole::Drafts));
    assert_eq!(drafts.added.len(), 1);
    assert_eq!(drafts.added[0].id, draft);
    assert!(drafts.added[0].flags.contains(Flags::DRAFT | Flags::SEEN));
    assert!(matches!(block_on(backend.fetch_body(&MessageId("no-such-email".into()))), Err(MailError::NotFound(_))));
}

#[test]
fn sending_goes_through_email_submission() {
    let Some(server) = stalwart() else { return };
    let user = server.user("jsend");
    let backend = backend(server, &user);
    let me = Address::new("Me", &user.address);
    let raw = format!("From: Me <{0}>\r\nTo: Me <{0}>\r\nSubject: by JMAP\r\nDate: Mon, 23 Nov 2026 12:30:00 +0000\r\nMessage-ID: <jmap-send@test>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSent with EmailSubmission/set.\r\n", user.address);
    block_on(backend.send(raw.as_bytes(), &me, std::slice::from_ref(&me))).unwrap();

    let inbox = with_role(&backend, FolderRole::Inbox);
    let deadline = Instant::now() + Duration::from_secs(30);
    let arrived = loop {
        let delta = full_sync(&backend, &inbox);
        if let Some(found) = delta.added.into_iter().find(|e| e.subject == "by JMAP") { break found; }
        assert!(Instant::now() < deadline, "the message never arrived");
        std::thread::sleep(Duration::from_millis(300));
    };
    assert_eq!(arrived.preview, "Sent with EmailSubmission/set.");
    let sent = full_sync(&backend, &with_role(&backend, FolderRole::Sent));
    assert_eq!(subjects(&sent.added), ["by JMAP"]);
    assert!(sent.added[0].flags.contains(Flags::SEEN));
}

#[test]
fn the_engine_runs_over_jmap() {
    let Some(server) = stalwart() else { return };
    use mail_core::machine::account_sync;
    use mail_store::Store;
    use ravenglass_engine::{Command, Config, Engine, Event, ReaderView};
    let user = server.user("jengine");
    server.imap(&user, &["CREATE Archive"]);
    server.deliver(&user, "hello", "From the engine test.");
    server.deliver(&user, "second", "Another one.");
    let account = AccountId::from(ACCOUNT);
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config { undo_send_secs: 0.0 });
    engine.add_backend(Arc::new(backend(server, &user)));

    let events = block_on(engine.handle(Command::OpenAccount(account.clone()), 1000.0));
    assert!(events.contains(&Event::SyncState { account: account.clone(), state: account_sync::State::Idle }), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, Event::Error { .. })), "{events:?}");
    let folders = events.iter().find_map(|e| match e { Event::FolderList { folders, .. } => Some(folders.clone()), _ => None }).expect("a folder list");
    let inbox = folders.iter().find(|f| f.role == FolderRole::Inbox).unwrap().clone();
    assert_eq!((inbox.total, inbox.unread), (2, 2));
    let page = match block_on(engine.handle(Command::LoadRange { folder: inbox.id.clone(), offset: 0, limit: 10 }, 1000.0)).remove(0) {
        Event::Envelopes { envelopes, .. } => envelopes,
        other => panic!("{other:?}"),
    };
    assert_eq!(subjects(&page), ["hello", "second"]);
    let hello = find(&page, "hello").clone();
    assert_eq!(hello.preview, "From the engine test.");

    let opened = block_on(engine.handle(Command::OpenMessage { id: hello.id.clone(), view: ReaderView::Text }, 1000.0));
    assert!(matches!(&opened[0], Event::Body { id, body } if *id == hello.id && body.subject == "hello"), "{opened:?}");
    let flagged = block_on(engine.handle(Command::SetFlags { ids: vec![hello.id.clone()], add: Flags::FLAGGED, remove: Flags::empty() }, 1001.0));
    assert!(flagged.iter().any(|e| matches!(e, Event::Applied { sent: true, .. })), "{flagged:?}");
    let moved = block_on(engine.handle(Command::MoveToRole { ids: vec![hello.id.clone()], role: FolderRole::Archive }, 1002.0));
    assert!(moved.iter().any(|e| matches!(e, Event::Applied { sent: true, .. })), "{moved:?}");
    assert!(imap_flags(server, &user, "Archive", "hello").contains("\\Flagged"));

    server.deliver(&user, "third", "Arrived later.");
    let refreshed = block_on(engine.handle(Command::Refresh(account.clone()), 1003.0));
    assert!(refreshed.contains(&Event::SyncState { account: account.clone(), state: account_sync::State::Idle }), "{refreshed:?}");
    let page = match block_on(engine.handle(Command::LoadRange { folder: inbox.id.clone(), offset: 0, limit: 10 }, 1003.0)).remove(0) {
        Event::Envelopes { envelopes, .. } => envelopes,
        other => panic!("{other:?}"),
    };
    assert_eq!(subjects(&page), ["second", "third"]);
}

#[test]
fn two_accounts_on_one_server_keep_their_own_folders_and_mail() {
    let Some(server) = stalwart() else { return };
    use mail_store::Store;
    use ravenglass_engine::{Command, Config, Engine, Event};
    // Stalwart gives both accounts the same mailbox and message ids; the store's ids are global.
    let (alice, bob) = (server.user("jalice"), server.user("jbob"));
    server.deliver(&alice, "for alice", "x");
    server.deliver(&bob, "for bob", "x");
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config::default());
    engine.add_backend(Arc::new(backend_as(server, &alice, "alice")));
    engine.add_backend(Arc::new(backend_as(server, &bob, "bob")));
    let mut inboxes = Vec::new();
    for account in ["alice", "bob"] {
        let events = block_on(engine.handle(Command::OpenAccount(AccountId::from(account)), 1000.0));
        let folders = events.iter().find_map(|e| match e { Event::FolderList { folders, .. } => Some(folders.clone()), _ => None }).expect("a folder list");
        inboxes.push(folders.into_iter().find(|f| f.role == FolderRole::Inbox).unwrap().id);
    }
    assert_ne!(inboxes[0], inboxes[1]);
    for (inbox, subject) in inboxes.iter().zip(["for alice", "for bob"]) {
        match block_on(engine.handle(Command::LoadRange { folder: inbox.clone(), offset: 0, limit: 10 }, 1000.0)).remove(0) {
            Event::Envelopes { envelopes, .. } => assert_eq!(subjects(&envelopes), [subject]),
            other => panic!("{other:?}"),
        }
    }
}

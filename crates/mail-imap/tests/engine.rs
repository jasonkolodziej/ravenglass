//! The whole path the desktop app takes: `ravenglass_engine::Engine` over an `ImapBackend`,
//! driven by `runner::block_on` (no tokio runtime on the calling thread), against GreenMail.
//! Skipped unless `RAVENGLASS_IMAP_TEST=1`.

mod support;

use mail_core::machine::account_sync;
use mail_core::*;
use mail_store::Store;
use ravenglass_engine::runner::{self, block_on};
use ravenglass_engine::{Command, Config, Engine, Event, ReaderView};
use std::sync::Arc;
use std::time::Duration;
use support::*;

fn envelopes(engine: &mut Engine, folder: &FolderId) -> Vec<Envelope> {
    match block_on(engine.handle(Command::LoadRange { folder: folder.clone(), offset: 0, limit: 50 }, 1000.0)).remove(0) {
        Event::Envelopes { envelopes, .. } => envelopes,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_engine_syncs_reads_and_acts_through_the_backend() {
    let Some(rig) = Rig::new("engine") else { return };
    rig.imap(&["CREATE \"Archive\""]);
    rig.deliver("second", "The newer of the two.");
    let Rig { user, backend } = rig;
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config { undo_send_secs: 0.0 });
    engine.add_backend(Arc::new(backend));

    // OpenAccount: connect, list folders, sync each, settle in Idle.
    let events = block_on(engine.handle(Command::OpenAccount(account()), 1000.0));
    assert!(events.contains(&Event::SyncState { account: account(), state: account_sync::State::Idle }), "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, Event::Error { .. })), "{events:?}");
    let folders = events.iter().find_map(|e| match e { Event::FolderList { folders, .. } => Some(folders.clone()), _ => None }).expect("a folder list");
    let inbox_folder = folders.iter().find(|f| f.role == FolderRole::Inbox).unwrap();
    assert_eq!((inbox_folder.id.clone(), inbox_folder.total, inbox_folder.unread), (inbox(), 2, 2));

    // LoadRange: the envelopes are in the store.
    let page = envelopes(&mut engine, &inbox());
    assert_eq!(page.len(), 2);
    let second = page.iter().find(|e| e.subject == "second").unwrap().clone();
    assert_eq!(second.preview, "The newer of the two.");

    // OpenMessage: the body comes from the server and renders.
    let opened = block_on(engine.handle(Command::OpenMessage { id: second.id.clone(), view: ReaderView::Text }, 1000.0));
    assert!(matches!(&opened[0], Event::Body { id, body } if *id == second.id && body.subject == "second"), "{opened:?}");

    // SetFlags and MoveToRole reach the server.
    let flagged = block_on(engine.handle(Command::SetFlags { ids: vec![second.id.clone()], add: Flags::SEEN, remove: Flags::empty() }, 1000.0));
    assert!(flagged.iter().any(|e| matches!(e, Event::Applied { sent: true, .. })), "{flagged:?}");
    let seed = page.iter().find(|e| e.subject == "seed").unwrap().clone();
    let moved = block_on(engine.handle(Command::MoveToRole { ids: vec![seed.id.clone()], role: FolderRole::Archive }, 1000.0));
    assert!(moved.iter().any(|e| matches!(e, Event::Applied { sent: true, .. })), "{moved:?}");
    let on_server = imap(&user, &["SELECT INBOX", "UID FETCH 1:* (FLAGS)", "SELECT \"Archive\"", "UID SEARCH ALL"]);
    assert!(on_server.iter().any(|l| l.contains("FLAGS (\\Seen)") && l.contains("UID 2")), "{on_server:?}");
    assert!(on_server.contains(&"* SEARCH 1".to_owned()), "{on_server:?}");

    // Refresh (what an IDLE wake-up sends): new mail appears, and the moved message settles
    // under its new id in Archive.
    deliver(&user, &message("seed@localhost", &user, "third", "Arrived later."));
    let refreshed = block_on(engine.handle(Command::Refresh(account()), 1001.0));
    assert!(refreshed.contains(&Event::SyncState { account: account(), state: account_sync::State::Idle }), "{refreshed:?}");
    let subjects = |list: Vec<Envelope>| { let mut s: Vec<String> = list.into_iter().map(|e| e.subject).collect(); s.sort(); s };
    assert_eq!(subjects(envelopes(&mut engine, &inbox())), ["second", "third"]);
    assert_eq!(subjects(envelopes(&mut engine, &folder("Archive"))), ["seed"]);

    // Send: out through SMTP, back into the inbox on the next refresh.
    let me = Address::new("Me", &user);
    let sent = block_on(engine.handle(Command::Send { account: account(), raw: message(&user, &user, "from the engine", "Sent by the outbox.").into_bytes(), from: me.clone(), rcpt: vec![me] }, 1002.0));
    assert!(!sent.iter().any(|e| matches!(e, Event::Error { .. })), "{sent:?}");
    let ticked = block_on(engine.handle(Command::Tick, 1003.0));
    assert!(ticked.iter().any(|e| matches!(e, Event::OutboxState { state: mail_core::machine::outbox::State::Sent, .. })), "{ticked:?}");
    block_on(engine.handle(Command::Refresh(account()), 1004.0));
    assert!(subjects(envelopes(&mut engine, &inbox())).contains(&"from the engine".to_owned()));
}

#[test]
fn the_engine_thread_runs_the_backend() {
    let Some(rig) = Rig::new("runner") else { return };
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config::default());
    engine.add_backend(Arc::new(rig.backend));
    // `runner::spawn` is the desktop host: its thread parks while backend futures are pending.
    let handle = runner::spawn(engine, || {}).unwrap();
    assert!(handle.send(Command::OpenAccount(account())));
    let mut idle = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while let Ok(event) = handle.events.recv_timeout(Duration::from_secs(30)) {
        assert!(std::time::Instant::now() < deadline, "the account never settled");
        assert!(!matches!(event, Event::Error { .. }), "{event:?}");
        if event == (Event::SyncState { account: account(), state: account_sync::State::Idle }) { idle = true; }
        if matches!(event, Event::FolderList { .. }) { break; }
    }
    assert!(idle, "the account reached Idle");
    assert!(handle.send(Command::LoadRange { folder: inbox(), offset: 0, limit: 10 }));
    match handle.events.recv_timeout(Duration::from_secs(30)).unwrap() {
        Event::Envelopes { envelopes, .. } => assert_eq!(envelopes.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["seed"]),
        other => panic!("{other:?}"),
    }
    handle.shutdown();
}

#[test]
fn a_wrong_password_ends_in_auth_failed() {
    let Some(rig) = Rig::with("engine-auth", |c| c.credential = mail_imap::Credential::Password("wrong".into())) else { return };
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config::default());
    engine.add_backend(Arc::new(rig.backend));
    let events = block_on(engine.handle(Command::OpenAccount(account()), 1000.0));
    assert!(events.contains(&Event::SyncState { account: account(), state: account_sync::State::AuthFailed }), "{events:?}");
}

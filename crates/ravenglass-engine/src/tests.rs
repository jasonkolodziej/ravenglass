use crate::runner::block_on;
use crate::*;
use mail_core::machine::{account_sync, image_load, outbox};
use mail_core::*;
use mail_mock::MockBackend;
use mail_store::Store;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

/// A mock account whose network can be cut and whose server can refuse moves.
struct Flaky { inner: MockBackend, offline: AtomicBool, refuse_moves: AtomicBool }

impl Flaky {
    fn new(account: &str, count: usize) -> Arc<Self> {
        Arc::new(Self { inner: MockBackend::new(account, 46, count), offline: AtomicBool::new(false), refuse_moves: AtomicBool::new(false) })
    }

    fn check(&self) -> Result<()> {
        if self.offline.load(Ordering::SeqCst) { Err(MailError::Network("offline".into())) } else { Ok(()) }
    }
}

impl MailBackend for Flaky {
    fn account(&self) -> &AccountId { self.inner.account() }
    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> { Box::pin(async move { self.check()?; self.inner.list_folders().await }) }
    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> {
        Box::pin(async move { self.check()?; self.inner.sync_folder(folder, since).await })
    }
    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> { Box::pin(async move { self.check()?; self.inner.fetch_body(msg).await }) }
    fn set_flags<'a>(&'a self, msgs: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { self.check()?; self.inner.set_flags(msgs, add, remove).await })
    }
    fn move_to<'a>(&'a self, msgs: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            self.check()?;
            if self.refuse_moves.load(Ordering::SeqCst) { return Err(MailError::Server("NO [CANNOT] move".into())); }
            self.inner.move_to(msgs, to).await
        })
    }
    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> { self.inner.append_draft(raw) }
    fn send<'a>(&'a self, raw: &'a [u8], from: &'a Address, rcpt: &'a [Address]) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { self.check()?; self.inner.send(raw, from, rcpt).await })
    }
    fn idle<'a>(&'a self, folder: &'a FolderId, events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> { self.inner.idle(folder, events) }
}

struct Rig { engine: Engine, backend: Arc<Flaky>, account: AccountId, now: f64 }

impl Rig {
    fn new() -> Self {
        let backend = Flaky::new("iCloud", 60);
        let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config::default());
        engine.add_backend(backend.clone());
        Self { engine, backend, account: AccountId::from("iCloud"), now: 1000.0 }
    }

    fn synced() -> Self {
        let mut rig = Self::new();
        let events = rig.run(Command::OpenAccount(rig.account.clone()));
        assert!(events.contains(&Event::SyncState { account: rig.account.clone(), state: account_sync::State::Idle }), "{events:?}");
        rig
    }

    fn run(&mut self, cmd: Command) -> Vec<Event> { block_on(self.engine.handle(cmd, self.now)) }

    fn inbox(&self) -> FolderId { FolderId("iCloud/Inbox".into()) }

    fn page(&mut self) -> Vec<Envelope> {
        match self.run(Command::LoadRange { folder: self.inbox(), offset: 0, limit: 10 }).remove(0) {
            Event::Envelopes { envelopes, .. } => envelopes,
            other => panic!("{other:?}"),
        }
    }

    fn folder(&self, name: &str) -> Folder {
        self.engine.store().folders(&self.account).unwrap().into_iter().find(|f| f.name == name).unwrap()
    }
}

fn applied(events: &[Event]) -> (OpId, bool) {
    events.iter().find_map(|e| match e { Event::Applied { op, sent } => Some((*op, *sent)), _ => None }).unwrap_or_else(|| panic!("no Applied in {events:?}"))
}

#[test]
fn open_account_syncs_every_folder_inbox_first_and_reports_counts() {
    let mut rig = Rig::new();
    let events = rig.run(Command::OpenAccount(rig.account.clone()));
    let states: Vec<_> = events.iter().filter_map(|e| match e { Event::SyncState { state, .. } => Some(state.clone()), _ => None }).collect();
    assert_eq!(states.first(), Some(&account_sync::State::Connecting { attempt: 0 }));
    assert_eq!(states.last(), Some(&account_sync::State::Idle));
    let Some(folders) = events.iter().rev().find_map(|e| match e { Event::FolderList { folders, .. } => Some(folders), _ => None }) else { panic!("{events:?}") };
    assert!(events.contains(&Event::Watch { account: rig.account.clone(), folder: rig.inbox() }), "an idle account asks for its Inbox to be watched");
    assert_eq!(folders.iter().map(|f| f.total).sum::<u32>(), 60);
    assert_eq!(rig.page().len(), 10);
    // A second pass finds nothing new and settles again.
    let again = rig.run(Command::Refresh(rig.account.clone()));
    assert!(again.contains(&Event::SyncState { account: rig.account.clone(), state: account_sync::State::Idle }));
    assert_eq!(rig.folder("Inbox").total, folders[0].total);
}

#[test]
fn archive_applies_at_once_reaches_the_server_and_undoes_on_both() {
    let mut rig = Rig::synced();
    let first = rig.page().remove(0);
    let before = rig.folder("Inbox").total;
    let events = rig.run(Command::MoveToRole { ids: vec![first.id.clone()], role: FolderRole::Archive });
    let (op, sent) = applied(&events);
    assert!(sent);
    assert_eq!(rig.folder("Inbox").total, before - 1);
    let on_server = |rig: &Rig, folder: &str| {
        let delta = block_on(rig.backend.inner.sync_folder(&FolderId(format!("iCloud/{folder}")), &SyncToken::default())).unwrap();
        delta.added.iter().any(|e| e.id == first.id)
    };
    assert!(on_server(&rig, "Archive"));
    assert!(rig.run(Command::Undo(op)).contains(&Event::Undone { op }));
    assert_eq!(rig.folder("Inbox").total, before);
    assert!(on_server(&rig, "Inbox"));
    assert!(matches!(rig.run(Command::Undo(op)).as_slice(), [Event::Error { .. }]), "an action undoes once");
}

#[test]
fn offline_actions_are_journaled_and_replayed_in_order_on_reconnect() {
    let mut rig = Rig::synced();
    let ids: Vec<MessageId> = rig.page().into_iter().take(2).map(|e| e.id).collect();
    rig.backend.offline.store(true, Ordering::SeqCst);
    let (flag, sent) = applied(&rig.run(Command::SetFlags { ids: ids.clone(), add: Flags::FLAGGED, remove: Flags::empty() }));
    assert!(!sent);
    let (moved, _) = applied(&rig.run(Command::MoveToRole { ids: vec![ids[0].clone()], role: FolderRole::Archive }));
    assert!(rig.page().iter().all(|e| e.id != ids[0]), "the list changes while offline");
    assert_eq!(rig.engine.store().journal_pending().unwrap().len(), 2);
    // The next sync pass fails and backs off; its timer reconnects and replays the journal.
    let failed = rig.run(Command::Refresh(rig.account.clone()));
    let due = failed.iter().find_map(|e| match e { Event::WakeAt(t) => Some(*t), _ => None }).expect("a backoff timer");
    rig.backend.offline.store(false, Ordering::SeqCst);
    rig.now = due;
    let events = rig.run(Command::Tick);
    let replayed: Vec<OpId> = events.iter().filter_map(|e| match e { Event::Applied { op, sent: true } => Some(*op), _ => None }).collect();
    assert_eq!(replayed, [flag, moved]);
    assert!(rig.engine.store().journal_pending().unwrap().is_empty());
    let archived = block_on(rig.backend.inner.sync_folder(&FolderId("iCloud/Archive".into()), &SyncToken::default())).unwrap();
    assert!(archived.added.iter().any(|e| e.id == ids[0] && e.flags.contains(Flags::FLAGGED)));
}

#[test]
fn undo_while_offline_drops_the_journal_entry_and_never_reaches_the_server() {
    let mut rig = Rig::synced();
    let first = rig.page().remove(0);
    rig.backend.offline.store(true, Ordering::SeqCst);
    let (op, _) = applied(&rig.run(Command::MoveToRole { ids: vec![first.id.clone()], role: FolderRole::Archive }));
    rig.run(Command::Undo(op));
    assert!(rig.engine.store().journal_pending().unwrap().is_empty());
    assert_eq!(rig.page()[0].id, first.id);
}

#[test]
fn a_refused_action_rolls_back_locally() {
    let mut rig = Rig::synced();
    let first = rig.page().remove(0);
    rig.backend.refuse_moves.store(true, Ordering::SeqCst);
    let events = rig.run(Command::MoveToRole { ids: vec![first.id.clone()], role: FolderRole::Archive });
    assert!(events.iter().any(|e| matches!(e, Event::Reverted { .. })) && !events.iter().any(|e| matches!(e, Event::Applied { .. })), "{events:?}");
    assert_eq!(rig.page()[0].id, first.id);
    assert!(rig.engine.store().journal_pending().unwrap().is_empty());
}

#[test]
fn opening_a_message_fetches_once_then_reads_the_cache() {
    let mut rig = Rig::synced();
    let first = rig.page().remove(0);
    let open = Command::OpenMessage { id: first.id.clone(), view: ReaderView::Html };
    let Some(Event::Body { body, .. }) = rig.run(open.clone()).pop() else { panic!("no body") };
    assert_eq!(body.subject, first.subject);
    assert!(body.html.is_none() && !body.text.lines.is_empty(), "mock mail is plain text");
    assert_eq!(body.images, image_load::State::Loaded);
    rig.backend.offline.store(true, Ordering::SeqCst);
    assert!(matches!(rig.run(open).pop(), Some(Event::Body { .. })), "the cached body opens offline");
}

#[test]
fn send_waits_for_the_undo_window_and_cancel_stops_it() {
    let mut rig = Rig::synced();
    let send = |rig: &mut Rig| {
        let cmd = Command::Send { account: rig.account.clone(), raw: b"Subject: hi\r\n\r\nbody".to_vec(), from: Address::new("J", "j@example.com"), rcpt: vec![Address::new("O", "o@example.com")] };
        let events = rig.run(cmd);
        let id = events.iter().find_map(|e| match e { Event::OutboxState { id, state: outbox::State::Queued { .. } } => Some(*id), _ => None }).unwrap();
        assert!(events.contains(&Event::WakeAt(rig.now + 10.0)));
        id
    };
    let cancelled = send(&mut rig);
    assert!(rig.run(Command::CancelSend(cancelled)).contains(&Event::OutboxState { id: cancelled, state: outbox::State::Cancelled }));
    let kept = send(&mut rig);
    rig.now += 5.0;
    assert!(rig.run(Command::Tick).is_empty(), "nothing is sent inside the undo window");
    rig.now += 5.0;
    assert!(rig.run(Command::Tick).contains(&Event::OutboxState { id: kept, state: outbox::State::Sent }));
    assert_eq!(rig.backend.inner.sent.lock().unwrap().len(), 1);
    assert!(matches!(rig.run(Command::CancelSend(kept)).as_slice(), [Event::Error { .. }]), "a sent message cannot be recalled");
}

#[test]
fn failed_send_stays_queued_for_retry() {
    let mut rig = Rig::synced();
    rig.engine = { let mut e = Engine::new(Store::open_in_memory().unwrap(), Config { undo_send_secs: 0.0 }); e.add_backend(rig.backend.clone()); e };
    rig.run(Command::OpenAccount(rig.account.clone()));
    rig.backend.offline.store(true, Ordering::SeqCst);
    let queued = rig.run(Command::Send { account: rig.account.clone(), raw: vec![], from: Address::new("J", "j@example.com"), rcpt: vec![] });
    let id = queued.iter().find_map(|e| match e { Event::OutboxState { id, .. } => Some(*id), _ => None }).unwrap();
    assert!(rig.run(Command::Tick).iter().any(|e| matches!(e, Event::OutboxState { state: outbox::State::Failed { .. }, .. })));
    rig.backend.offline.store(false, Ordering::SeqCst);
    assert!(rig.run(Command::RetrySend(id)).contains(&Event::OutboxState { id, state: outbox::State::Sent }));
}

#[test]
fn allowing_images_for_a_sender_is_remembered() {
    let mut rig = Rig::synced();
    let first = rig.page().remove(0);
    rig.run(Command::AllowImages { message: first.id.clone(), scope: image_load::Scope::Once });
    assert!(!rig.engine.store().images_allowed_from(&first.from.email).unwrap());
    rig.run(Command::AllowImages { message: first.id.clone(), scope: image_load::Scope::Sender });
    assert!(rig.engine.store().images_allowed_from(&first.from.email).unwrap());
}

#[test]
fn commands_and_events_survive_the_wire() {
    let mut rig = Rig::synced();
    let cmd = Command::MoveToRole { ids: vec![rig.page().remove(0).id], role: FolderRole::Archive };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
    for event in rig.run(cmd) {
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }
}

#[test]
fn runner_thread_serves_commands_and_delivers_timers() {
    let backend = Flaky::new("iCloud", 20);
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config { undo_send_secs: 0.05 });
    engine.add_backend(backend.clone());
    let (woke, wakes) = std::sync::mpsc::channel();
    let handle = runner::spawn(engine, move || { let _ = woke.send(()); }).unwrap();
    let account = AccountId::from("iCloud");
    assert!(handle.send(Command::OpenAccount(account.clone())));
    assert!(handle.send(Command::Send { account, raw: vec![], from: Address::new("J", "j@example.com"), rcpt: vec![] }));
    let wait = std::time::Duration::from_secs(10);
    let mut sent = false;
    while !sent {
        let event = handle.events.recv_timeout(wait).expect("the outbox timer fires without another command");
        sent = matches!(event, Event::OutboxState { state: outbox::State::Sent, .. });
    }
    assert!(wakes.try_recv().is_ok());
    handle.shutdown();
    assert_eq!(backend.inner.sent.lock().unwrap().len(), 1);
}

/// A backend whose first Inbox message is the HTML newsletter fixture.
struct WithHtml(MockBackend);

impl MailBackend for WithHtml {
    fn account(&self) -> &AccountId { self.0.account() }
    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> { self.0.list_folders() }
    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> { self.0.sync_folder(folder, since) }
    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> {
        if msg.0.ends_with("/00000") {
            let bytes = include_bytes!("../../../fixtures/emails/02-newsletter-tables.eml").to_vec();
            return Box::pin(async move { Ok(RawMessage { id: msg.clone(), bytes }) });
        }
        self.0.fetch_body(msg)
    }
    fn set_flags<'a>(&'a self, msgs: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>> { self.0.set_flags(msgs, add, remove) }
    fn move_to<'a>(&'a self, msgs: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>> { self.0.move_to(msgs, to) }
    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> { self.0.append_draft(raw) }
    fn send<'a>(&'a self, raw: &'a [u8], from: &'a Address, rcpt: &'a [Address]) -> BoxFut<'a, Result<()>> { self.0.send(raw, from, rcpt) }
    fn idle<'a>(&'a self, folder: &'a FolderId, events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> { self.0.idle(folder, events) }
}

#[test]
fn html_mail_is_laid_out_tiled_hit_tested_and_resized_through_the_engine() {
    use mail_core::machine::html_view;
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config::default());
    engine.add_backend(Arc::new(WithHtml(MockBackend::new("iCloud", 46, 20))));
    let run = |engine: &mut Engine, cmd| block_on(engine.handle(cmd, 0.0));
    run(&mut engine, Command::OpenAccount(AccountId::from("iCloud")));
    let (view, html, plain) = (ViewId(1), MessageId::from("iCloud/00000"), MessageId::from("iCloud/00001"));
    let render = |message: &MessageId| Command::RenderHtml { view, message: message.clone(), options: HtmlOptions::default() };

    // Without a render thread the reader is told to use the text view.
    assert!(matches!(run(&mut engine, render(&html)).as_slice(), [Event::HtmlFailed { .. }]));
    engine.set_renderer(Renderer::spawn().unwrap());

    let Some(Event::Body { body, .. }) = run(&mut engine, Command::OpenMessage { id: html.clone(), view: ReaderView::Html }).pop() else { panic!() };
    assert_eq!(body.images, mail_core::machine::image_load::State::Blocked { remote: 1 });
    let Some(Event::HtmlLaidOut { info, .. }) = run(&mut engine, render(&html)).pop() else { panic!("no layout") };
    assert_eq!((info.width, info.tiles, info.refused_requests), (680, 1, 0));
    assert_eq!(engine.html.state(view), Some(&html_view::State::Rastering { width: 680, height: info.height, pending: 1 }));
    let Some(Event::HtmlTile { tile, .. }) = run(&mut engine, Command::HtmlTile { view, index: 0 }).pop() else { panic!("no tile") };
    assert_eq!((tile.width, tile.height), (info.width, info.height));
    assert_eq!(engine.html.state(view), Some(&html_view::State::Ready { width: 680, height: info.height }));

    let hovers: Vec<Event> = (0..info.height).step_by(8).flat_map(|y| block_on(engine.handle(Command::HtmlPointer { view, x: 340.0, y: y as f32 }, 0.0))).collect();
    assert!(hovers.iter().any(|e| matches!(e, Event::LinkHover { link: Some(l), .. } if l.text == "Unsubscribe")), "the centred footer link");
    assert!(hovers.iter().any(|e| matches!(e, Event::LinkHover { link: None, .. })));

    let Some(Event::HtmlLaidOut { info: narrow, .. }) = run(&mut engine, Command::ResizeHtml { view, width: 320 }).pop() else { panic!("no relayout") };
    assert_eq!(narrow.width, 320);
    assert!(run(&mut engine, Command::ResizeHtml { view, width: 320 }).is_empty(), "the same width keeps the layout");

    // A plain-text message has no HTML view, and closing a view stops its events.
    assert!(matches!(run(&mut engine, render(&plain)).as_slice(), [Event::HtmlFailed { failure: HtmlFailure::Empty, .. }]));
    run(&mut engine, render(&html));
    run(&mut engine, Command::CloseHtml(view));
    assert!(run(&mut engine, Command::HtmlTile { view, index: 0 }).is_empty());
}

#[test]
fn a_reply_is_drafted_autosaved_sent_through_the_outbox_and_closed() {
    use mail_core::machine::compose::AUTOSAVE_SECS;
    let mut rig = Rig::synced();
    rig.engine.set_identity(&rig.account.clone(), Address::new("Jason", "jason@example.com"));
    let original = rig.page().remove(0);
    let id = DraftId::from("d1");
    let source = DraftSource::Answer { message: original.id.clone(), kind: ReplyKind::Reply };
    let Some(Event::Draft(mut draft)) = rig.run(Command::StartDraft { id: id.clone(), account: rig.account.clone(), source }).pop() else { panic!("no draft") };
    assert_eq!((draft.to.as_slice(), draft.from.email.as_str()), (std::slice::from_ref(&original.from), "jason@example.com"));
    assert!(draft.subject.starts_with("Re: ") && draft.body.contains("> "), "{draft:?}");

    draft.body.insert_str(0, "Thanks, on it.");
    let edited = rig.run(Command::EditDraft(draft.clone()));
    assert!(edited.contains(&Event::DraftSaved { id: id.clone(), on_server: false }) && edited.contains(&Event::WakeAt(rig.now + AUTOSAVE_SECS)));
    assert_eq!(rig.engine.store().drafts().unwrap().len(), 1, "saved locally at once");
    assert!(rig.run(Command::Tick).is_empty(), "the server copy waits for a quiet period");
    rig.now += AUTOSAVE_SECS;
    assert!(rig.run(Command::Tick).contains(&Event::DraftSaved { id: id.clone(), on_server: true }));

    let sent = rig.run(Command::SendDraft(id.clone()));
    let Some(Event::DraftClosed { outbox: Some(outbox), .. }) = sent.last() else { panic!("{sent:?}") };
    assert!(rig.engine.store().drafts().unwrap().is_empty());
    rig.now += 10.0;
    assert!(rig.run(Command::Tick).contains(&Event::OutboxState { id: *outbox, state: outbox::State::Sent }));
    let raw = rig.backend.inner.sent.lock().unwrap().pop().unwrap();
    let wire = mail_render::parse(&raw).unwrap();
    assert_eq!((wire.to.as_slice(), wire.text.unwrap().starts_with("Thanks, on it.")), (std::slice::from_ref(&original.from), true));
    assert!(matches!(rig.run(Command::SendDraft(id)).as_slice(), [Event::Error { .. }]), "a draft sends once");
}

#[test]
fn drafts_need_a_recipient_survive_a_restart_and_can_be_discarded() {
    let mut rig = Rig::synced();
    let id = DraftId::from("d2");
    let Some(Event::Draft(mut draft)) = rig.run(Command::StartDraft { id: id.clone(), account: rig.account.clone(), source: DraftSource::Blank }).pop() else { panic!() };
    draft.subject = "Unfinished".into();
    rig.run(Command::EditDraft(draft.clone()));
    assert!(matches!(rig.run(Command::SendDraft(id.clone())).as_slice(), [Event::Error { .. }]), "no recipient");
    // A new engine over the same store finds the draft again.
    let Rig { engine, backend, .. } = rig;
    let mut restarted = Engine::new(engine.into_store(), Config::default());
    restarted.add_backend(backend);
    let loaded = block_on(restarted.handle(Command::LoadDrafts, 0.0));
    assert_eq!(loaded, [Event::Draft(draft)]);
    assert_eq!(block_on(restarted.handle(Command::DiscardDraft(id.clone()), 0.0)), [Event::DraftClosed { id, outbox: None }]);
    assert!(block_on(restarted.handle(Command::LoadDrafts, 0.0)).is_empty());
}

/// A mock account that pushes "new mail" once, the first time its Inbox is watched.
struct Pushy { inner: MockBackend, pushed: AtomicBool, syncs: std::sync::atomic::AtomicU32 }

impl MailBackend for Pushy {
    fn account(&self) -> &AccountId { self.inner.account() }
    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> { self.inner.list_folders() }
    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> {
        if folder.0.ends_with("/Inbox") { self.syncs.fetch_add(1, Ordering::SeqCst); }
        self.inner.sync_folder(folder, since)
    }
    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> { self.inner.fetch_body(msg) }
    fn set_flags<'a>(&'a self, msgs: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>> { self.inner.set_flags(msgs, add, remove) }
    fn move_to<'a>(&'a self, msgs: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>> { self.inner.move_to(msgs, to) }
    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> { self.inner.append_draft(raw) }
    fn send<'a>(&'a self, raw: &'a [u8], from: &'a Address, rcpt: &'a [Address]) -> BoxFut<'a, Result<()>> { self.inner.send(raw, from, rcpt) }
    fn idle<'a>(&'a self, folder: &'a FolderId, events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            if !self.pushed.swap(true, Ordering::SeqCst) { let _ = events.send(IdleEvent::Exists(folder.clone())); }
            Ok(())
        })
    }
}

#[test]
fn a_push_from_the_server_triggers_a_sync_without_any_command() {
    let backend = Arc::new(Pushy { inner: MockBackend::new("iCloud", 46, 20), pushed: AtomicBool::new(false), syncs: 0.into() });
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config::default());
    engine.add_backend(backend.clone());
    let handle = runner::spawn(engine, || {}).unwrap();
    assert!(handle.send(Command::OpenAccount(AccountId::from("iCloud"))));
    let mut idles = 0;
    while idles < 2 {
        let event = handle.events.recv_timeout(std::time::Duration::from_secs(10)).expect("the push starts a second sync pass");
        assert!(!matches!(event, Event::Watch { .. } | Event::WakeAt(_)), "runner-only events stay in the runner");
        if matches!(event, Event::SyncState { state: account_sync::State::Idle, .. }) { idles += 1; }
    }
    handle.shutdown();
    assert_eq!(backend.syncs.load(Ordering::SeqCst), 2);
}

#[test]
fn an_idle_account_polls_its_other_folders_on_a_timer() {
    let mut rig = Rig::synced();
    assert!(rig.run(Command::Tick).is_empty(), "not yet due");
    rig.now += POLL_SECS;
    let events = rig.run(Command::Tick);
    assert!(events.contains(&Event::SyncState { account: rig.account.clone(), state: account_sync::State::Idle }), "{events:?}");
    assert!(events.contains(&Event::WakeAt(rig.now + POLL_SECS)));
}

#[test]
fn remote_images_load_only_after_the_user_allows_them() {
    use std::sync::Mutex;
    let mut engine = Engine::new(Store::open_in_memory().unwrap(), Config::default());
    engine.add_backend(Arc::new(WithHtml(MockBackend::new("iCloud", 46, 20))));
    engine.set_renderer(Renderer::spawn().unwrap());
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    engine.set_image_fetcher({ let asked = asked.clone(); Arc::new(move |url: &str| { asked.lock().unwrap().push(url.to_owned()); None }) });
    let run = |engine: &mut Engine, cmd| block_on(engine.handle(cmd, 0.0));
    run(&mut engine, Command::OpenAccount(AccountId::from("iCloud")));
    let (view, message) = (ViewId(1), MessageId::from("iCloud/00000"));
    let render = Command::RenderHtml { view, message: message.clone(), options: HtmlOptions::default() };

    run(&mut engine, render.clone());
    assert!(asked.lock().unwrap().is_empty(), "nothing is fetched for a blocked message");
    let allowed = run(&mut engine, Command::AllowImages { message: message.clone(), scope: mail_core::machine::image_load::Scope::Once });
    assert!(matches!(allowed.as_slice(), [Event::ImagesAllowed { .. }, Event::HtmlLaidOut { .. }]), "the open view is laid out again: {allowed:?}");
    assert_eq!(*asked.lock().unwrap(), ["https://cdn.example.com/hero.png"], "the hero image, never the tracking pixel");
    run(&mut engine, Command::CloseHtml(view));
    assert_eq!(run(&mut engine, Command::AllowImages { message, scope: mail_core::machine::image_load::Scope::Once }).len(), 1, "a closed view is not rendered");
}

#[test]
fn actions_that_change_nothing_say_so_and_a_removed_account_is_forgotten() {
    let mut rig = Rig::synced();
    let first = rig.page().remove(0);
    let archive = Command::MoveToRole { ids: vec![first.id.clone()], role: FolderRole::Archive };
    applied(&rig.run(archive.clone()));
    assert_eq!(rig.run(archive), [Event::Unchanged], "already archived");
    assert_eq!(rig.run(Command::SetFlags { ids: vec![MessageId::from("nope")], add: Flags::SEEN, remove: Flags::empty() }), [Event::Unchanged]);

    assert_eq!(rig.run(Command::RemoveAccount(rig.account.clone())), [Event::AccountRemoved(rig.account.clone())]);
    assert!(rig.engine.store().folders(&rig.account).unwrap().is_empty());
    assert!(matches!(rig.run(Command::OpenAccount(rig.account.clone())).as_slice(), [Event::Error { .. }]));
}

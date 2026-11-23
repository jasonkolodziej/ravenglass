//! The engine proper: one `handle(Command) -> Vec<Event>` step at a time. It owns the store, the
//! backends, the sync and outbox machines and the journal, and it never reads a clock: the caller
//! passes `now`, and timers come back as [`Event::WakeAt`] then arrive as [`Command::Tick`].

use crate::api::*;
use crate::ops::Op;
use mail_core::machine::{Effect, account_sync, image_load, outbox};
use mail_core::*;
use mail_render::{Body, Prefer};
use mail_store::{Query, Store};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

/// Envelopes per page (architecture §13).
pub const PAGE: u32 = 200;
/// Folders without push are checked this often while an account is idle.
pub const POLL_SECS: f64 = 300.0;
/// Past actions that can still be undone.
const UNDO_DEPTH: usize = 50;

#[derive(Clone, Debug)]
pub struct Config {
    /// How long a sent message waits in the outbox (0–30 s).
    pub undo_send_secs: f64,
}

impl Default for Config {
    fn default() -> Self { Self { undo_send_secs: 10.0 } }
}

pub(crate) struct Account {
    pub(crate) backend: Arc<dyn MailBackend>,
    /// The account's own address: the sender of its drafts.
    pub(crate) identity: Address,
    sync: account_sync::State,
    /// Folders still to sync in this pass, Inbox first.
    queue: VecDeque<FolderId>,
    total: u32,
    /// When an idle account next checks the folders that have no push.
    next_poll: f64,
}

struct Outgoing { account: AccountId, raw: Vec<u8>, from: Address, rcpt: Vec<Address>, state: outbox::State }

/// An undoable action. `pending`: still in the journal, the server has not accepted it yet.
struct Done { op: Op, pending: bool }

pub struct Engine {
    pub(crate) html: crate::html::Html,
    pub(crate) drafts: crate::drafts::Drafts,
    pub(crate) store: Store,
    config: Config,
    pub(crate) accounts: BTreeMap<AccountId, Account>,
    outbox: BTreeMap<OutboxId, Outgoing>,
    next_outbox: u64,
    history: BTreeMap<OpId, Done>,
    /// Fetches one remote image the user allowed; `None` if it cannot be had.
    images: Option<ImageFetcher>,
    /// Messages whose images were allowed for this session only.
    allowed_once: std::collections::BTreeSet<MessageId>,
    /// What each HTML view shows, to render it again when its images are allowed.
    shown: BTreeMap<ViewId, (MessageId, mail_blitz::Options)>,
}

/// Fetch one image by URL. It must send no cookies and no referrer, and bound its own time.
pub type ImageFetcher = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;
/// Remote images fetched for one message at most.
const MAX_REMOTE_IMAGES: usize = 40;

pub(crate) type Events = Vec<Event>;
/// A command step; the error text becomes an [`Event::Error`].
pub(crate) type Outcome<T = ()> = std::result::Result<T, String>;

impl Engine {
    pub fn new(store: Store, config: Config) -> Self {
        Self { html: crate::html::Html::default(), drafts: crate::drafts::Drafts::default(), store, config, accounts: BTreeMap::new(), outbox: BTreeMap::new(), next_outbox: 1, history: BTreeMap::new(), images: None, allowed_once: Default::default(), shown: BTreeMap::new() }
    }

    pub fn add_backend(&mut self, backend: Arc<dyn MailBackend>) {
        let identity = Address { name: None, email: backend.account().0.clone() };
        let account = Account { backend: backend.clone(), identity, sync: account_sync::State::Disconnected, queue: VecDeque::new(), total: 0, next_poll: f64::INFINITY };
        self.accounts.insert(backend.account().clone(), account);
    }

    /// The name and address an account sends as. Defaults to the account id as the address.
    pub fn set_identity(&mut self, account: &AccountId, identity: Address) {
        if let Some(a) = self.accounts.get_mut(account) { a.identity = identity; }
    }

    /// How allowed remote images are fetched. Without it they stay placeholders.
    pub fn set_image_fetcher(&mut self, fetcher: ImageFetcher) { self.images = Some(fetcher); }

    /// Give the engine a render thread. Without one, `RenderHtml` answers `HtmlFailed` and the
    /// reader shows the text view.
    pub fn set_renderer(&mut self, renderer: mail_blitz::Renderer) { self.html.renderer = Some(renderer); }

    pub fn backend(&self, account: &AccountId) -> Option<Arc<dyn MailBackend>> { self.accounts.get(account).map(|a| a.backend.clone()) }

    pub fn store(&self) -> &Store { &self.store }

    /// Stop the engine and keep its store.
    pub fn into_store(self) -> Store { self.store }

    /// Run one command to completion. Errors become [`Event::Error`]; the engine keeps going.
    pub async fn handle(&mut self, cmd: Command, now: f64) -> Events {
        let mut out = Events::new();
        let context = command_name(&cmd);
        if let Err(message) = self.dispatch(cmd, now, &mut out).await {
            out.push(Event::Error { context: context.into(), message });
        }
        out
    }

    async fn dispatch(&mut self, cmd: Command, now: f64, out: &mut Events) -> Outcome {
        match cmd {
            Command::OpenAccount(account) => self.sync_event(&account, account_sync::Event::Start, now, out).await,
            Command::Refresh(account) => self.sync_event(&account, account_sync::Event::IdleWoke, now, out).await,
            Command::LoadRange { folder, offset, limit } => {
                let envelopes = self.store.envelopes(&folder, offset, limit.min(PAGE)).map_err(text)?;
                out.push(Event::Envelopes { folder, offset, envelopes });
                Ok(())
            }
            Command::OpenMessage { id, view } => self.open_message(id, view, out).await,
            Command::SetFlags { ids, add, remove } => {
                let Some((account, envelopes)) = self.resolve(&ids)? else { return Ok(()) };
                let before = envelopes.into_iter().map(|e| (e.id, e.flags)).collect();
                self.apply(Op::SetFlags { account, before, add, remove }, out).await
            }
            Command::Move { ids, to } => {
                let Some((account, envelopes)) = self.resolve(&ids)? else { return Ok(()) };
                let from: Vec<_> = envelopes.into_iter().filter(|e| e.folder != to).map(|e| (e.id, e.folder)).collect();
                if from.is_empty() { return Ok(()); }
                self.apply(Op::Move { account, from, to }, out).await
            }
            Command::MoveToRole { ids, role } => {
                let Some((account, envelopes)) = self.resolve(&ids)? else { return Ok(()) };
                let folders = self.store.folders(&account).map_err(text)?;
                let to = folders.into_iter().find(|f| f.role == role).ok_or_else(|| format!("this account has no {role:?} folder"))?.id;
                let from: Vec<_> = envelopes.into_iter().filter(|e| e.folder != to).map(|e| (e.id, e.folder)).collect();
                if from.is_empty() { return Ok(()); }
                self.apply(Op::Move { account, from, to }, out).await
            }
            Command::Undo(op) => self.undo(op, out).await,
            Command::Search { query, account, limit } => {
                let envelopes = self.store.search(&Query::parse(&query), account.as_ref(), limit.min(PAGE)).map_err(text)?;
                out.push(Event::SearchResults { query, envelopes });
                Ok(())
            }
            Command::Send { account, raw, from, rcpt } => self.queue_send(account, raw, from, rcpt, now, out).await.map(|_| ()),
            Command::StartDraft { id, account, source } => self.start_draft(id, account, source, out).await,
            Command::EditDraft(draft) => self.edit_draft(*draft, now, out),
            Command::SendDraft(id) => self.send_draft(id, now, out).await,
            Command::DiscardDraft(id) => self.discard_draft(id, out),
            Command::LoadDrafts => self.load_drafts(out),
            Command::CancelSend(id) => self.outbox_event(id, outbox::Event::Undo, out).await,
            Command::RetrySend(id) => self.outbox_event(id, outbox::Event::Retry, out).await,
            Command::AllowImages { message, scope } => {
                self.allowed_once.insert(message.clone());
                if scope == image_load::Scope::Sender
                    && let Some(envelope) = self.store.envelope(&message).map_err(text)?
                {
                    self.store.allow_images_from(&envelope.from.email).map_err(text)?;
                }
                out.push(Event::ImagesAllowed { message: message.clone() });
                // Views already showing the message are laid out again, now with its images.
                let showing: Vec<(ViewId, mail_blitz::Options)> = self.shown.iter().filter(|(_, (m, _))| *m == message).map(|(v, (_, o))| (*v, o.clone())).collect();
                for (view, options) in showing { self.render_html(view, message.clone(), options, out).await?; }
                Ok(())
            }
            Command::CompleteAddress { prefix, limit } => {
                let addresses = self.store.complete_address(&prefix, limit).map_err(text)?;
                out.push(Event::Addresses { prefix, addresses });
                Ok(())
            }
            Command::RenderHtml { view, message, options } => self.render_html(view, message, options, out).await,
            Command::ResizeHtml { view, width } => { self.html.resize(view, width, out); Ok(()) }
            Command::HtmlTile { view, index } => { self.html.tile(view, index, out); Ok(()) }
            Command::HtmlPointer { view, x, y } => { self.html.pointer(view, x, y, out); Ok(()) }
            Command::CloseHtml(view) => { self.shown.remove(&view); self.html.close(view); Ok(()) }
            Command::Tick => {
                let accounts: Vec<AccountId> = self.accounts.keys().cloned().collect();
                for account in accounts {
                    let due = self.accounts.get(&account).is_some_and(|a| a.sync == account_sync::State::Idle && now >= a.next_poll);
                    let event = if due { account_sync::Event::IdleWoke } else { account_sync::Event::Timer(now) };
                    self.sync_event(&account, event, now, out).await?;
                }
                let queued: Vec<OutboxId> = self.outbox.keys().copied().collect();
                for id in queued { self.outbox_event(id, outbox::Event::Timer(now), out).await?; }
                self.draft_timers(now, out).await;
                Ok(())
            }
        }
    }

    // ── sync ───────────────────────────────────────────────────────────────────────────────

    /// Feed one event to an account's sync machine and perform effects until it settles.
    async fn sync_event(&mut self, id: &AccountId, first: account_sync::Event, now: f64, out: &mut Events) -> Outcome {
        let mut events = VecDeque::from([first]);
        while let Some(event) = events.pop_front() {
            let account = self.accounts.get_mut(id).ok_or("unknown account")?;
            let before = account.sync.clone();
            let effects = account_sync::step(&mut account.sync, event, now);
            if account.sync != before { out.push(Event::SyncState { account: id.clone(), state: account.sync.clone() }); }
            for effect in effects {
                match self.sync_effect(id, effect, now, out).await {
                    Ok(Some(next)) => events.push_back(next),
                    Ok(None) => {}
                    Err(e) => {
                        // The next pass starts over rather than resuming a half-synced queue.
                        if let Some(account) = self.accounts.get_mut(id) { account.queue.clear(); }
                        events.push_back(account_sync::Event::Failed(e));
                    }
                }
            }
        }
        Ok(())
    }

    async fn sync_effect(&mut self, id: &AccountId, effect: Effect, now: f64, out: &mut Events) -> mail_core::Result<Option<account_sync::Event>> {
        let account = self.accounts.get_mut(id).ok_or_else(|| MailError::NotFound("account".into()))?;
        let backend = account.backend.clone();
        Ok(match effect {
            // Backends connect on first use, so the first request doubles as the connection test.
            Effect::Connect => Some(account_sync::Event::Connected),
            Effect::Authenticate => {
                let folders = backend.list_folders().await?;
                self.store.upsert_folders(&folders).map_err(store_error)?;
                Some(account_sync::Event::Authenticated)
            }
            Effect::SyncNext => {
                if account.queue.is_empty() {
                    let mut folders = self.store.folders(id).map_err(store_error)?;
                    folders.sort_by_key(|f| f.role != FolderRole::Inbox);
                    account.total = folders.len() as u32;
                    account.queue = folders.into_iter().map(|f| f.id).collect();
                }
                let total = account.total;
                let Some(folder) = account.queue.pop_front() else { return Ok(Some(account_sync::Event::FolderSynced { total: 0 })) };
                let since = self.store.sync_token(&folder).map_err(store_error)?;
                let delta = backend.sync_folder(&folder, &since).await?;
                self.store.apply_delta(id, &folder, &delta).map_err(store_error)?;
                Some(account_sync::Event::FolderSynced { total })
            }
            Effect::StartIdle => {
                account.next_poll = now + POLL_SECS;
                self.replay_journal(id, out).await;
                self.push_folders(id, out);
                if let Some(inbox) = self.store.folders(id).map_err(store_error)?.into_iter().find(|f| f.role == FolderRole::Inbox) {
                    out.push(Event::Watch { account: id.clone(), folder: inbox.id });
                }
                out.push(Event::WakeAt(now + POLL_SECS));
                None
            }
            Effect::WakeAt(t) => { out.push(Event::WakeAt(t)); None }
            Effect::Notify(line) => { out.push(Event::Notice(line)); None }
            Effect::Send { .. } | Effect::ShowUndo { .. } => None,
        })
    }

    fn push_folders(&self, account: &AccountId, out: &mut Events) {
        match self.store.folders(account) {
            Ok(folders) => out.push(Event::FolderList { account: account.clone(), folders }),
            Err(e) => out.push(Event::Error { context: "folders".into(), message: e.to_string() }),
        }
    }

    // ── actions, journal, undo ─────────────────────────────────────────────────────────────

    /// The envelopes behind a command's ids, which must share one account. `None` if none exist.
    fn resolve(&self, ids: &[MessageId]) -> Outcome<Option<(AccountId, Vec<Envelope>)>> {
        let mut envelopes = Vec::with_capacity(ids.len());
        let mut account = None;
        for id in ids {
            let (Some(envelope), Some(owner)) = (self.store.envelope(id).map_err(text)?, self.store.account_of(id).map_err(text)?) else { continue };
            if account.get_or_insert_with(|| owner.clone()) != &owner { return Err("messages belong to different accounts".into()); }
            envelopes.push(envelope);
        }
        Ok(account.map(|a| (a, envelopes)))
    }

    /// Optimistic apply: local store first, then the journal, then the server if it is reachable.
    async fn apply(&mut self, op: Op, out: &mut Events) -> Outcome {
        op.apply(&mut self.store).map_err(text)?;
        let seq = self.store.journal_push(&serde_json::to_string(&op).map_err(text)?).map_err(text)?;
        let id = OpId(seq);
        let account = op.account().clone();
        // A refusal has already rolled the change back and reported `Reverted`.
        if let Some(sent) = self.replay(id, &op, out).await {
            out.push(Event::Applied { op: id, sent });
            self.remember(id, op, !sent);
        }
        self.push_folders(&account, out);
        Ok(())
    }

    /// Try one journaled op against the server. `Some(true)`: accepted. `Some(false)`: unreachable,
    /// it stays journaled. `None`: refused, so the local change is rolled back.
    async fn replay(&mut self, id: OpId, op: &Op, out: &mut Events) -> Option<bool> {
        let account = self.accounts.get(op.account())?;
        if !matches!(account.sync, account_sync::State::Idle | account_sync::State::Syncing { .. }) { return Some(false); }
        match op.replay(account.backend.as_ref()).await {
            Ok(()) => { let _ = self.store.journal_remove(id.0); Some(true) }
            Err(MailError::Network(_)) => Some(false),
            Err(refused) => {
                let _ = op.revert(&mut self.store);
                let _ = self.store.journal_remove(id.0);
                self.history.remove(&id);
                out.push(Event::Reverted { op: id, reason: refused.to_string() });
                None
            }
        }
    }

    async fn replay_journal(&mut self, account: &AccountId, out: &mut Events) {
        let Ok(pending) = self.store.journal_pending() else { return };
        for (seq, json) in pending {
            let Ok(op) = serde_json::from_str::<Op>(&json) else { let _ = self.store.journal_remove(seq); continue };
            if op.account() != account { continue; }
            match self.replay(OpId(seq), &op, out).await {
                Some(true) => {
                    if let Some(done) = self.history.get_mut(&OpId(seq)) { done.pending = false; }
                    out.push(Event::Applied { op: OpId(seq), sent: true });
                }
                // Still offline: keep the order, try again on the next pass.
                Some(false) => break,
                None => {}
            }
        }
    }

    fn remember(&mut self, id: OpId, op: Op, pending: bool) {
        self.history.insert(id, Done { op, pending });
        while self.history.len() > UNDO_DEPTH { self.history.pop_first(); }
    }

    async fn undo(&mut self, id: OpId, out: &mut Events) -> Outcome {
        let done = self.history.get(&id).ok_or("nothing to undo")?;
        if done.pending {
            // The server never saw it: drop it from the journal.
            self.store.journal_remove(id.0).map_err(text)?;
        } else if let Some(account) = self.accounts.get(done.op.account()) {
            done.op.replay_inverse(account.backend.as_ref()).await.map_err(|e| e.to_string())?;
        }
        let Some(done) = self.history.remove(&id) else { return Ok(()) };
        done.op.revert(&mut self.store).map_err(text)?;
        out.push(Event::Undone { op: id });
        self.push_folders(done.op.account(), out);
        Ok(())
    }

    // ── reader ─────────────────────────────────────────────────────────────────────────────

    async fn render_html(&mut self, view: ViewId, message: MessageId, options: mail_blitz::Options, out: &mut Events) -> Outcome {
        let raw = self.raw(&message).await?;
        let parsed = mail_render::parse(&raw.bytes).map_err(text)?;
        let html = parsed.html.as_deref().map(|h| mail_render::sanitize(h, &message).html);
        let sender_allowed = match &parsed.from { Some(from) => self.store.images_allowed_from(&from.email).map_err(text)?, None => false };
        let mut remote = std::collections::HashMap::new();
        if let (true, Some(fetch), Some(html)) = (sender_allowed || self.allowed_once.contains(&message), &self.images, &html) {
            let mut budget = mail_blitz::MAX_MESSAGE_IMAGE_BYTES;
            for url in mail_render::remote_images(html).into_iter().take(MAX_REMOTE_IMAGES) {
                let Some(bytes) = fetch(&url).filter(|b| b.len() <= mail_blitz::MAX_IMAGE_BYTES.min(budget)) else { continue };
                budget -= bytes.len();
                remote.insert(url, bytes);
            }
        }
        self.shown.insert(view, (message, options.clone()));
        self.html.open(view, html, mail_blitz::Images { inline: parsed.inline, remote }, options, out);
        Ok(())
    }

    /// A message's raw bytes: from the cache, else from the server.
    pub(crate) async fn raw(&mut self, id: &MessageId) -> Outcome<RawMessage> {
        if let Some(raw) = self.store.body(id).map_err(text)? { return Ok(raw); }
        let account = self.store.account_of(id).map_err(text)?.ok_or("unknown message")?;
        let backend = self.accounts.get(&account).ok_or("unknown account")?.backend.clone();
        backend.fetch_body(id).await.map_err(|e| e.to_string())
    }

    async fn open_message(&mut self, id: MessageId, view: ReaderView, out: &mut Events) -> Outcome {
        let fresh = self.store.body(&id).map_err(text)?.is_none();
        let raw = self.raw(&id).await?;
        let parsed = mail_render::parse(&raw.bytes).map_err(text)?;
        if fresh { self.store.put_body(&raw, &parsed.index_text(&id)).map_err(text)?; }
        let prefer = match view { ReaderView::Html => Prefer::Html, ReaderView::Text => Prefer::Plain };
        let (text_body, html) = match parsed.body(&id, prefer) {
            Body::Text(t) => (t, None),
            Body::Html { html, text } => (text, Some(html)),
        };
        let sender_allowed = match &parsed.from { Some(from) => self.store.images_allowed_from(&from.email).map_err(text)?, None => false };
        let images = image_load::open(html.as_ref().map_or(0, |h| h.remote_images), sender_allowed).0;
        let body = BodyView { subject: parsed.subject, from: parsed.from, to: parsed.to, cc: parsed.cc, date: parsed.date, attachments: parsed.attachments, text: text_body, html, images };
        out.push(Event::Body { id, body: Box::new(body) });
        Ok(())
    }

    // ── outbox ─────────────────────────────────────────────────────────────────────────────

    pub(crate) async fn queue_send(&mut self, account: AccountId, raw: Vec<u8>, from: Address, rcpt: Vec<Address>, now: f64, out: &mut Events) -> Outcome<OutboxId> {
        if !self.accounts.contains_key(&account) { return Err("unknown account".into()); }
        let id = OutboxId(self.next_outbox);
        self.next_outbox += 1;
        let (state, effects) = outbox::queue(now, self.config.undo_send_secs.clamp(0.0, 30.0));
        self.outbox.insert(id, Outgoing { account, raw, from, rcpt, state: state.clone() });
        out.push(Event::OutboxState { id, state });
        self.outbox_effects(id, effects, out).await;
        Ok(id)
    }

    async fn outbox_event(&mut self, id: OutboxId, first: outbox::Event, out: &mut Events) -> Outcome {
        let item = self.outbox.get_mut(&id).ok_or("no such outgoing message")?;
        let before = item.state.clone();
        let effects = outbox::step(&mut item.state, first);
        if item.state != before { out.push(Event::OutboxState { id, state: item.state.clone() }); }
        self.outbox_effects(id, effects, out).await;
        Ok(())
    }

    async fn outbox_effects(&mut self, id: OutboxId, effects: Vec<Effect>, out: &mut Events) {
        let mut effects = VecDeque::from(effects);
        while let Some(effect) = effects.pop_front() {
            match effect {
                Effect::WakeAt(t) => out.push(Event::WakeAt(t)),
                Effect::Notify(line) => out.push(Event::Notice(line)),
                Effect::Send { .. } => {
                    let Some(item) = self.outbox.get(&id) else { continue };
                    let Some(backend) = self.accounts.get(&item.account).map(|a| a.backend.clone()) else { continue };
                    let result = backend.send(&item.raw, &item.from, &item.rcpt).await;
                    let event = match result { Ok(()) => outbox::Event::SendOk, Err(e) => outbox::Event::SendErr(e) };
                    let Some(item) = self.outbox.get_mut(&id) else { continue };
                    effects.extend(outbox::step(&mut item.state, event));
                    out.push(Event::OutboxState { id, state: item.state.clone() });
                }
                _ => {}
            }
        }
        // Finished messages leave the queue; failed ones stay for `RetrySend`.
        if self.outbox.get(&id).is_some_and(|i| matches!(i.state, outbox::State::Sent | outbox::State::Cancelled)) { self.outbox.remove(&id); }
    }
}

pub(crate) fn text(e: impl std::fmt::Display) -> String { e.to_string() }

fn store_error(e: mail_store::StoreError) -> MailError { MailError::Server(format!("local store: {e}")) }

fn command_name(cmd: &Command) -> &'static str {
    match cmd {
        Command::OpenAccount(_) => "open account", Command::Refresh(_) => "refresh", Command::LoadRange { .. } => "load messages",
        Command::OpenMessage { .. } => "open message", Command::SetFlags { .. } => "set flags", Command::Move { .. } | Command::MoveToRole { .. } => "move",
        Command::Undo(_) => "undo", Command::Search { .. } => "search", Command::Send { .. } => "send", Command::CancelSend(_) => "cancel send",
        Command::RetrySend(_) => "retry send", Command::AllowImages { .. } => "allow images", Command::CompleteAddress { .. } => "complete address", Command::Tick => "timers",
        Command::StartDraft { .. } | Command::EditDraft(_) | Command::LoadDrafts => "draft", Command::SendDraft(_) => "send draft", Command::DiscardDraft(_) => "discard draft",
        Command::RenderHtml { .. } | Command::ResizeHtml { .. } | Command::HtmlTile { .. } | Command::HtmlPointer { .. } | Command::CloseHtml(_) => "render HTML",
    }
}

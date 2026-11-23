//! Engine events folded into `AppState`. The window (events from the engine thread, in `poll`)
//! and headless runs (events returned inline) share [`AppState::fold`], so a snapshot shows
//! what the window would.

use crate::grid::{Grid, Rect};
use crate::link::Link;
use crate::modes::Mode;
use crate::state::*;
use mail_core::machine::account_sync;
use mail_core::{AccountId, FolderId};
use ravenglass_engine::{Command, Event, PAGE, ReaderView};
use ravenglass_theme::Theme;
use std::cell::Cell;
use std::collections::VecDeque;

impl AppState {
    fn with(link: Link, wall: WallClock, boot: Boot) -> AppState {
        let theme = Theme::night();
        let mailbox = |account: &str, name: &str| Source::Mailbox { account: AccountId::from(account), name: name.into() };
        // The demo set: the plan's workspace bar over the two mock accounts.
        let workspaces = vec![
            Workspace::new(1, "iCloud", mailbox("iCloud", "Inbox")),
            Workspace::new(2, "Gmail", mailbox("Gmail", "Inbox")),
            Workspace::new(3, "Archive", mailbox("iCloud", "Archive")),
            Workspace::new(5, "receipts", mailbox("iCloud", "Receipts")),
            Workspace::new(6, "from:github", Source::Search("from:github".into())),
        ];
        let grid = Grid::from_metrics(theme.ui.font_size.unwrap_or(14.0), theme.ui.line_height.unwrap_or(1.35), 8.4, 2.0);
        let frame = Frame { grid, panes: Rect::new(0, 2, 160, 42), narrow: false };
        AppState {
            theme,
            mode: Mode::default(),
            workspaces,
            active: 0,
            mail: Mail::default(),
            reader_view: ReaderView::Html,
            toast: None,
            undo: Vec::new(),
            undoing: None,
            restore: None,
            pending: VecDeque::new(),
            fx: Vec::new(),
            link,
            boot,
            waker: None,
            wall,
            clock: 0.0,
            frame: Cell::new(frame),
        }
    }

    /// The window's state: empty until the engine starts after the first frame.
    pub fn windowed() -> AppState { AppState::with(Link::Off, WallClock::System, Boot::Cold) }

    /// Fully loaded at once from an inline engine on the mock's fixed clock: headless scenes,
    /// scripts and tests.
    pub fn demo_inline() -> AppState {
        let now = mail_mock::NOW;
        let (link, failure) = match Link::inline(now as f64) {
            Ok(link) => (link, None),
            Err(reason) => (Link::Off, Some(reason)),
        };
        let mut s = AppState::with(link, WallClock::Fixed(now), Boot::Started);
        if let Some(reason) = failure { s.error(format!("engine: {reason}")); }
        s.open_accounts();
        s
    }

    /// Start the engine thread (the window, after its first frame).
    pub fn start_engine(&mut self) {
        let Some(waker) = self.waker.clone() else { return };
        self.link = Link::start(move || waker());
        self.boot = Boot::Started;
        self.open_accounts();
    }

    /// The accounts the workspaces name, in slot order.
    pub fn accounts(&self) -> Vec<AccountId> {
        let mut out: Vec<AccountId> = Vec::new();
        for ws in &self.workspaces {
            if let Source::Mailbox { account, .. } = &ws.source && !out.contains(account) { out.push(account.clone()); }
        }
        out
    }

    pub fn open_accounts(&mut self) {
        for account in self.accounts() { self.dispatch(Command::OpenAccount(account)); }
    }

    /// Send a command; fold whatever comes back at once (the inline engine).
    pub fn dispatch(&mut self, cmd: Command) {
        for event in self.link.send(cmd) { self.fold(event); }
    }

    /// Fold events that arrived from the engine thread. True when any did.
    pub fn pump(&mut self) -> bool {
        let events = self.link.drain();
        let any = !events.is_empty();
        for event in events { self.fold(event); }
        any
    }

    pub fn fold(&mut self, event: Event) {
        match event {
            Event::FolderList { account, folders } => {
                self.mail.folders.insert(account, folders);
                self.resolve_folders();
            }
            Event::Envelopes { folder, offset, envelopes } => {
                let list = self.mail.lists.entry(folder).or_default();
                list.truncate(offset as usize);
                list.extend(envelopes);
                self.settle_selection();
            }
            Event::SyncState { account, state } => {
                if state == account_sync::State::Idle {
                    self.mail.synced_at = Some(self.wall.now());
                    self.rerun_searches();
                }
                self.mail.sync.insert(account, state);
            }
            Event::Applied { op, .. } => {
                // The journal replays an op when the account comes back; only the first answer
                // belongs to a pending action.
                if self.undo.iter().any(|u| u.op == op) { return; }
                if let Some(done) = self.pending.pop_front() && !done.silent {
                    self.toast(format!("{} {} · u undo", done.label, done.ids.len()));
                    self.undo.push(Undoable { op, label: done.label, ids: done.ids });
                }
                self.reload();
            }
            Event::Reverted { op, .. } => {
                self.undo.retain(|u| u.op != op);
                self.error("the server refused that; it was put back");
                self.reload();
            }
            Event::Undone { .. } => {
                if let Some(entry) = self.undoing.take() {
                    self.toast(format!("undone: {} {}", entry.label, entry.ids.len()));
                    self.restore = entry.ids.into_iter().next();
                }
                self.reload();
            }
            Event::SearchResults { query, envelopes } => {
                for ws in &mut self.workspaces {
                    if let Some(search) = &mut ws.search && search.query == query {
                        search.results = envelopes.clone();
                        search.pending = false;
                    }
                }
                self.settle_selection();
            }
            Event::Body { id, body } => self.mail.bodies.put(id, *body),
            Event::Notice(line) => self.toast(line),
            Event::Error { context, message } => {
                match context.as_str() {
                    "move" | "set flags" => { self.pending.pop_front(); }
                    "undo" => self.undoing = None,
                    _ => {}
                }
                self.error(format!("{context}: {message}"));
            }
            Event::OutboxState { .. } | Event::ImagesAllowed { .. } | Event::Addresses { .. } | Event::WakeAt(_) | Event::Watch { .. } => {}
            // Compose and the HTML surface are not drawn yet; their events have nowhere to go.
            Event::Draft(_) | Event::DraftSaved { .. } | Event::DraftClosed { .. } => {}
            Event::HtmlLaidOut { .. } | Event::HtmlTile { .. } | Event::LinkHover { .. } | Event::HtmlFailed { .. } => {}
        }
    }

    /// Point each mailbox workspace at its folder once the account's folder list is known, and
    /// load the first page of any that just resolved.
    fn resolve_folders(&mut self) {
        let mut load: Vec<FolderId> = Vec::new();
        for ws in &mut self.workspaces {
            let Source::Mailbox { account, name } = &ws.source else { continue };
            if ws.folder.is_some() { continue; }
            if let Some(folder) = self.mail.folder_named(account, name) {
                ws.folder = Some(folder.id.clone());
                if !load.contains(&folder.id) { load.push(folder.id.clone()); }
            }
        }
        for folder in load { self.dispatch(Command::LoadRange { folder, offset: 0, limit: PAGE }); }
    }

    /// Load the active workspace's list if it was never loaded.
    pub fn ensure_loaded(&mut self) {
        if self.ws().search.is_some() { return; }
        let Some(folder) = self.ws().folder.clone() else { return };
        if !self.mail.lists.contains_key(&folder) { self.dispatch(Command::LoadRange { folder, offset: 0, limit: PAGE }); }
    }

    /// The engine changed the cache (an action, an undo): fetch every list on show again.
    fn reload(&mut self) {
        let folders: Vec<FolderId> = self.mail.lists.keys().cloned().collect();
        for folder in folders { self.dispatch(Command::LoadRange { folder, offset: 0, limit: PAGE }); }
        self.rerun_searches();
    }

    fn rerun_searches(&mut self) {
        let mut queries: Vec<String> = Vec::new();
        for ws in &self.workspaces {
            if let Some(search) = &ws.search && !queries.contains(&search.query) { queries.push(search.query.clone()); }
        }
        for query in queries { self.dispatch(Command::Search { query, account: None, limit: PAGE }); }
    }

    /// Keep every selection on a row, and put it on a message an undo brought back.
    fn settle_selection(&mut self) {
        let restore = self.restore.clone();
        let active = self.active;
        let mut reveal = false;
        for i in 0..self.workspaces.len() {
            let len = self.rows_of(&self.workspaces[i]).len();
            let found = restore.as_ref().and_then(|id| self.rows_of(&self.workspaces[i]).iter().position(|e| &e.id == id));
            let ws = &mut self.workspaces[i];
            ws.sel = ws.sel.min(len.saturating_sub(1));
            if let Some(a) = ws.anchor { ws.anchor = Some(a.min(len.saturating_sub(1))); }
            if i == active && let Some(at) = found {
                ws.sel = at;
                reveal = true;
            }
        }
        if reveal {
            self.restore = None;
            if let (Some(pane), Some(id)) = (self.ws().panes.find(crate::panes::Kind::List), self.selected().map(|e| e.id.clone())) {
                self.fx.push(Fx::Reveal(AppState::row_key(pane, &id)));
            }
        }
        // The reader keeps an envelope of its own; refresh it so flags follow the store.
        let fresh = self.ws().open.as_ref().and_then(|open| self.rows().iter().find(|e| e.id == open.id).cloned());
        if let Some(fresh) = fresh { self.ws_mut().open = Some(fresh); }
    }
}

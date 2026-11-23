//! `AppState`: everything the views read. Handlers and engine events change it (`update.rs`);
//! views never do.

use crate::grid::{Grid, Rect};
use crate::link::Link;
use crate::modes::Mode;
use crate::panes::{Kind, PaneTree};
use mail_core::machine::account_sync;
use mail_core::{AccountId, Envelope, Folder, FolderId, FolderRole, MessageId, Timestamp};
use ravenglass_engine::{BodyView, OpId, ReaderView};
use ravenglass_theme::Theme;
use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

/// What a workspace slot shows (plan §0: an account, a mailbox or a saved search).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A mailbox by account and folder name, resolved when the account's folders arrive.
    Mailbox { account: AccountId, name: String },
    /// A saved search across accounts.
    Search(String),
}

/// A query and what it found so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Search {
    pub query: String,
    pub results: Vec<Envelope>,
    /// No answer for `query` has arrived yet.
    pub pending: bool,
}

pub struct Workspace {
    /// The digit that selects it (1–9).
    pub slot: u8,
    pub name: String,
    pub source: Source,
    /// The mailbox `source` resolved to.
    pub folder: Option<FolderId>,
    pub panes: PaneTree,
    /// The selected row of the list.
    pub sel: usize,
    /// VISUAL mode's other end of the range.
    pub anchor: Option<usize>,
    /// The message in the reader.
    pub open: Option<Envelope>,
    /// `zo`: folded quotes of the open message are shown.
    pub quotes_open: bool,
    /// A search shown in place of the mailbox.
    pub search: Option<Search>,
    /// The reader's scroll position in rows (`j`/`k` in the reader move it).
    pub reader_top: usize,
    /// How far the reader can scroll, in rows: written by the reader view when it lays out.
    pub reader_extent: Cell<usize>,
}

impl Workspace {
    pub fn new(slot: u8, name: &str, source: Source) -> Workspace {
        let search = match &source {
            Source::Search(q) => Some(Search { query: q.clone(), results: Vec::new(), pending: true }),
            Source::Mailbox { .. } => None,
        };
        Workspace { slot, name: name.into(), source, folder: None, panes: PaneTree::mail(), sel: 0, anchor: None, open: None, quotes_open: false, search, reader_top: 0, reader_extent: Cell::new(0) }
    }

    /// The selected rows as an index range (one row outside VISUAL mode).
    pub fn selected_range(&self) -> std::ops::RangeInclusive<usize> {
        let other = self.anchor.unwrap_or(self.sel);
        self.sel.min(other)..=self.sel.max(other)
    }
}

/// Bodies of recently opened messages, bounded by count and by size.
#[derive(Default)]
pub struct BodyCache {
    entries: VecDeque<(MessageId, Rc<BodyView>, usize)>,
    bytes: usize,
}

impl BodyCache {
    pub const MAX_ENTRIES: usize = 24;
    pub const MAX_BYTES: usize = 4 << 20;

    pub fn get(&self, id: &MessageId) -> Option<&Rc<BodyView>> { self.entries.iter().find(|(i, ..)| i == id).map(|(_, b, _)| b) }

    pub fn put(&mut self, id: MessageId, body: BodyView) {
        self.remove(&id);
        let size = body_size(&body);
        self.bytes += size;
        self.entries.push_back((id, Rc::new(body), size));
        while self.entries.len() > Self::MAX_ENTRIES || (self.bytes > Self::MAX_BYTES && self.entries.len() > 1) {
            if let Some((_, _, size)) = self.entries.pop_front() { self.bytes -= size; }
        }
    }

    pub fn remove(&mut self, id: &MessageId) {
        if let Some(at) = self.entries.iter().position(|(i, ..)| i == id)
            && let Some((_, _, size)) = self.entries.remove(at)
        {
            self.bytes -= size;
        }
    }

    pub fn len(&self) -> usize { self.entries.len() }
}

fn body_size(body: &BodyView) -> usize {
    let text: usize = body.text.lines.iter().map(|l| l.text().len() + 16).sum();
    text + body.html.as_ref().map_or(0, |h| h.html.len()) + body.subject.len()
}

/// What the engine has told us.
#[derive(Default)]
pub struct Mail {
    pub folders: BTreeMap<AccountId, Vec<Folder>>,
    pub lists: BTreeMap<FolderId, Vec<Envelope>>,
    pub bodies: BodyCache,
    pub sync: BTreeMap<AccountId, account_sync::State>,
    /// When an account last finished syncing.
    pub synced_at: Option<Timestamp>,
}

impl Mail {
    pub fn folder(&self, id: &FolderId) -> Option<&Folder> { self.folders.values().flatten().find(|f| &f.id == id) }

    pub fn folder_named(&self, account: &AccountId, name: &str) -> Option<&Folder> {
        self.folders.get(account)?.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    pub fn folder_with_role(&self, account: &AccountId, role: FolderRole) -> Option<&Folder> {
        self.folders.get(account)?.iter().find(|f| f.role == role)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    /// When it goes, on the app clock.
    pub due: f64,
}

/// A journaled action that `u` can take back.
#[derive(Clone, Debug, PartialEq)]
pub struct Undoable { pub op: OpId, pub label: String, pub ids: Vec<MessageId> }

/// A command sent to the engine whose `Applied` (or `Error`) has not come back yet.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingOp {
    pub label: String,
    pub ids: Vec<MessageId>,
    /// Bookkeeping the user did not ask for (marking a message read on open): no toast, no undo.
    pub silent: bool,
}

/// Things only a `Tx` can do, queued by state changes and carried out by the handler that ran them.
#[derive(Clone, Debug, PartialEq)]
pub enum Fx {
    /// Scroll a row into view.
    Reveal(String),
    /// Put a scroll view's top at this many rows down.
    ScrollRows(String, usize),
    ScrollTop(String),
    ScrollBottom(String),
    WakeAt(f64),
    /// The theme changed: let colours ease.
    ThemeChanged,
    Quit,
}

/// Engine start-up, after the first frame (plan §2 "Data flow").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boot {
    /// Nothing drawn yet.
    Cold,
    /// The first frame is out; start on the next poll.
    Armed,
    Started,
}

/// Wall time for date columns: fixed in headless runs so snapshots are reproducible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallClock { Fixed(Timestamp), System }

impl WallClock {
    pub fn now(self) -> Timestamp {
        match self {
            WallClock::Fixed(t) => t,
            WallClock::System => ravenglass_engine::runner::now() as Timestamp,
        }
    }
}

/// What the last view run laid out, for handlers that work in cells (pane splits and resizes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub grid: Grid,
    /// The pane area in cells.
    pub panes: Rect,
    /// Too narrow for two panes: only the focused pane shows.
    pub narrow: bool,
}

pub struct AppState {
    pub theme: Theme,
    pub mode: Mode,
    pub workspaces: Vec<Workspace>,
    /// Index into `workspaces`.
    pub active: usize,
    pub mail: Mail,
    /// The reader's view for HTML mail (`t` flips it). HTML is the product default.
    pub reader_view: ReaderView,
    pub toast: Option<Toast>,
    pub undo: Vec<Undoable>,
    /// The entry an `Undo` command in flight is taking back.
    pub undoing: Option<Undoable>,
    /// A message an undo brought back: select it when its list reloads.
    pub restore: Option<MessageId>,
    pub pending: VecDeque<PendingOp>,
    pub fx: Vec<Fx>,
    pub link: Link,
    pub boot: Boot,
    pub waker: Option<rune::Waker>,
    pub wall: WallClock,
    /// The app clock of the last poll or key (seconds; virtual when headless).
    pub clock: f64,
    pub frame: Cell<Frame>,
}

/// Below this many columns of pane area the layout collapses to one pane.
pub const NARROW_COLS: usize = 100;

impl AppState {
    pub fn ws(&self) -> &Workspace { &self.workspaces[self.active] }
    pub fn ws_mut(&mut self) -> &mut Workspace { &mut self.workspaces[self.active] }

    /// The rows the active workspace's list shows.
    pub fn rows(&self) -> &[Envelope] { self.rows_of(self.ws()) }

    pub fn rows_of<'a>(&'a self, ws: &'a Workspace) -> &'a [Envelope] {
        match &ws.search {
            Some(search) => &search.results,
            None => ws.folder.as_ref().and_then(|f| self.mail.lists.get(f)).map_or(&[], Vec::as_slice),
        }
    }

    pub fn selected(&self) -> Option<&Envelope> { self.rows().get(self.ws().sel) }

    /// The folder behind the active workspace, with its counts.
    pub fn folder(&self) -> Option<&Folder> { self.ws().folder.as_ref().and_then(|f| self.mail.folder(f)) }

    pub fn body(&self) -> Option<&Rc<BodyView>> { self.ws().open.as_ref().and_then(|e| self.mail.bodies.get(&e.id)) }

    /// The pane kind keys act on. A narrow window shows only the focused pane, so it is the same.
    pub fn focused_kind(&self) -> Kind { self.ws().panes.focused_kind() }

    pub fn row_key(pane: u32, id: &MessageId) -> String { format!("m:{pane}:{id}") }
    pub fn list_key(pane: u32) -> String { format!("list:{pane}") }
    pub fn reader_key(pane: u32) -> String { format!("reader:{pane}") }
}

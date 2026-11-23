//! Handlers: keys go through the mode machine, its effects run actions, and actions change
//! state or send engine commands. Nothing here blocks (the inline engine of headless runs aside)
//! or needs a `Tx`: what only a `Tx` can do is queued as [`Fx`].

use crate::actions::{self, Action};
use crate::keys::KeyPress;
use crate::modes::{self, Effect, Mode};
use crate::panes::{Axis, Dir, Kind};
use crate::state::*;
use mail_core::{Envelope, Flags, FolderRole, MessageId};
use ravenglass_engine::{Command, PAGE, ReaderView};

/// How long a toast stays, in seconds.
const TOAST_SECS: f64 = 6.0;
const ERROR_SECS: f64 = 9.0;

impl AppState {
    /// One key press. True when it did something.
    pub fn key(&mut self, press: KeyPress) -> bool {
        let before = self.mode.clone();
        let effects = modes::step(&mut self.mode, press);
        let acted = !effects.is_empty() || self.mode != before;
        for effect in effects { self.run(effect); }
        acted
    }

    fn run(&mut self, effect: Effect) {
        match effect {
            Effect::Act(action) => self.apply(action),
            Effect::AnchorSelection => { let sel = self.ws().sel; self.ws_mut().anchor = Some(sel); }
            Effect::ClearSelection => self.ws_mut().anchor = None,
            Effect::SearchChanged(query) => self.search(query),
            Effect::SearchAccepted(query) => if query.trim().is_empty() { self.clear_search(); },
            Effect::SearchCancelled => self.clear_search(),
            Effect::RunCommand(line) => match actions::parse_command(&line) {
                Ok(action) => self.perform(action),
                Err(reason) => self.error(reason),
            },
            Effect::RunPalette { query, selected } => {
                if let Some(entry) = actions::palette_matches(&query).get(selected) { self.perform(entry.action.clone()); }
            }
        }
    }

    /// Run an action from the command line or the palette: mode changes take the machine's own
    /// transitions, everything else is applied.
    pub fn perform(&mut self, action: Action) {
        match action {
            Action::Search | Action::CommandLine | Action::Palette | Action::Visual | Action::Compose => {
                for effect in modes::enter_action(&mut self.mode, action) { self.run(effect); }
            }
            action => self.apply(action),
        }
    }

    pub fn toast(&mut self, text: impl Into<String>) { self.show_toast(text.into(), false, TOAST_SECS); }
    pub fn error(&mut self, text: impl Into<String>) { self.show_toast(text.into(), true, ERROR_SECS); }

    fn show_toast(&mut self, text: String, error: bool, secs: f64) {
        let due = self.clock + secs;
        self.toast = Some(Toast { text, error, due });
        self.fx.push(Fx::WakeAt(due));
    }

    /// Timers: true when something visible changed.
    pub fn tick(&mut self, clock: f64) -> bool {
        self.clock = clock;
        if self.toast.as_ref().is_some_and(|t| t.due <= clock) {
            self.toast = None;
            return true;
        }
        false
    }

    // ── actions ────────────────────────────────────────────────────────────────────────────

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::MoveDown => self.step_selection(1),
            Action::MoveUp => self.step_selection(-1),
            Action::Top => self.jump_selection(false),
            Action::Bottom => self.jump_selection(true),
            Action::Open => self.open_selected(true),
            Action::Back => self.back(),
            Action::Archive => self.move_to_role(FolderRole::Archive, "archived"),
            Action::Delete => self.move_to_role(FolderRole::Trash, "deleted"),
            Action::ToggleRead => self.toggle_flag(Flags::SEEN, "marked read", "marked unread"),
            Action::ToggleFlag => self.toggle_flag(Flags::FLAGGED, "flagged", "unflagged"),
            Action::Undo => self.undo_last(),
            Action::ToggleView => {
                self.reader_view = match self.reader_view { ReaderView::Html => ReaderView::Text, ReaderView::Text => ReaderView::Html };
                self.toast(match self.reader_view { ReaderView::Html => "reader view: html", ReaderView::Text => "reader view: text" });
                if let Some(id) = self.ws().open.as_ref().map(|e| e.id.clone()) {
                    self.dispatch(Command::OpenMessage { id, view: self.reader_view });
                }
            }
            Action::LoadImages { always } => {
                use mail_core::machine::image_load::Scope;
                if let Some(message) = self.ws().open.as_ref().map(|e| e.id.clone()) {
                    self.dispatch(Command::AllowImages { message, scope: if always { Scope::Sender } else { Scope::Once } });
                    self.toast(if always { "images: always for this sender" } else { "images: loaded once" });
                }
            }
            Action::OpenQuotes => { let ws = self.ws_mut(); ws.quotes_open = !ws.quotes_open; }
            Action::Workspace(slot) => self.switch_workspace(slot),
            Action::NextTab | Action::PrevTab => self.cycle_tab(),
            Action::PaneFocus(dir) => { let area = self.frame.get().panes; let _ = self.ws_mut().panes.focus_dir(dir, area); }
            Action::PaneSplit(axis) => self.split(axis),
            Action::PaneClose => if let Err(e) = self.ws_mut().panes.close() { self.error(e.message()); },
            Action::PaneResize(dir) => {
                let area = self.frame.get().panes;
                let cells = if matches!(dir, Dir::Left | Dir::Right) { 2 } else { 1 };
                if let Err(e) = self.ws_mut().panes.resize(dir, cells, area) { self.error(e.message()); }
            }
            Action::PaneShow(kind) => { let ws = self.ws_mut(); let id = ws.panes.focus(); ws.panes.set_kind(id, kind); }
            Action::Theme(name) => self.set_theme(&name),
            Action::SearchFor(query) => self.search(query),
            Action::Mailbox(name) => self.go_to_mailbox(&name),
            Action::Refresh => for account in self.accounts() { self.dispatch(Command::Refresh(account)); },
            Action::Quit => self.fx.push(Fx::Quit),
            Action::Compose => self.toast("compose arrives with the compose pane · esc back"),
            Action::Reply | Action::ReplyAll | Action::Forward | Action::Send | Action::MoveTo | Action::GoToMailbox
            | Action::ToggleThread | Action::PaneTree => {
                let title = actions::catalog().iter().find(|e| e.action == action).map_or("this", |e| e.title);
                self.toast(format!("{}: not in this build yet", title.to_lowercase()));
            }
            // Mode changes belong to the mode machine (`perform`).
            Action::Search | Action::CommandLine | Action::Palette | Action::Visual => self.perform(action),
        }
    }

    fn list_pane(&self) -> Option<u32> { self.ws().panes.find(Kind::List) }

    fn reveal_selection(&mut self) {
        let Some(pane) = self.list_pane() else { return };
        if let Some(id) = self.selected().map(|e| e.id.clone()) { self.fx.push(Fx::Reveal(AppState::row_key(pane, &id))); }
    }

    /// Scroll the reader by whole rows, within what its last layout said it can scroll.
    fn scroll_reader(&mut self, by: isize) {
        let key = AppState::reader_key(self.ws().panes.focus());
        let ws = self.ws_mut();
        let max = ws.reader_extent.get() as isize;
        ws.reader_top = (ws.reader_top as isize).saturating_add(by).clamp(0, max) as usize;
        let top = ws.reader_top;
        self.fx.push(Fx::ScrollRows(key, top));
    }

    fn step_selection(&mut self, by: isize) {
        if self.focused_kind() == Kind::Reader {
            return self.scroll_reader(by * 3);
        }
        let len = self.rows().len();
        if len == 0 { return; }
        let ws = self.ws_mut();
        ws.sel = (ws.sel as isize + by).clamp(0, len as isize - 1) as usize;
        self.reveal_selection();
        self.load_more_if_near_end();
    }

    fn jump_selection(&mut self, end: bool) {
        if self.focused_kind() == Kind::Reader {
            return self.scroll_reader(if end { isize::MAX / 2 } else { isize::MIN / 2 });
        }
        let len = self.rows().len();
        if len == 0 { return; }
        self.ws_mut().sel = if end { len - 1 } else { 0 };
        self.reveal_selection();
        self.load_more_if_near_end();
    }

    /// Fetch the next page when the selection nears the end of what is loaded.
    fn load_more_if_near_end(&mut self) {
        let ws = self.ws();
        if ws.search.is_some() { return; }
        let Some(folder) = self.folder() else { return };
        let loaded = self.rows().len() as u32;
        if loaded >= folder.total || (ws.sel as u32) + 10 < loaded { return; }
        let folder = folder.id.clone();
        self.dispatch(Command::LoadRange { folder, offset: loaded, limit: PAGE });
    }

    /// Open the selected message in the reader. `by_user` marks it read and moves focus there.
    pub fn open_selected(&mut self, by_user: bool) {
        if self.focused_kind() == Kind::Reader && by_user { return; }
        let Some(envelope) = self.selected().cloned() else { return };
        let id = envelope.id.clone();
        let unread = envelope.is_unread();
        let area = self.frame.get().panes;
        let ws = self.ws_mut();
        ws.open = Some(envelope);
        ws.quotes_open = false;
        ws.reader_top = 0;
        if ws.panes.find(Kind::Reader).is_none() {
            match ws.panes.find(Kind::Empty) {
                Some(empty) => ws.panes.set_kind(empty, Kind::Reader),
                None => {
                    let list = ws.panes.focus();
                    let _ = ws.panes.split(Axis::Cols, Kind::Reader, area);
                    ws.panes.set_focus(list);
                }
            }
        }
        if by_user { ws.panes.focus_kind(Kind::Reader); }
        if let Some(reader) = self.ws().panes.find(Kind::Reader) { self.fx.push(Fx::ScrollTop(AppState::reader_key(reader))); }
        self.dispatch(Command::OpenMessage { id: id.clone(), view: self.reader_view });
        if by_user && unread {
            self.pending.push_back(PendingOp { label: "read".into(), ids: vec![id.clone()], silent: true });
            self.send_op(Command::SetFlags { ids: vec![id], add: Flags::SEEN, remove: Flags::empty() });
        }
    }

    fn back(&mut self) {
        if self.focused_kind() != Kind::List && self.ws_mut().panes.focus_kind(Kind::List) { return; }
        let saved = matches!(self.ws().source, Source::Search(_));
        if self.ws().search.is_some() && !saved {
            self.clear_search();
            return;
        }
        self.toast = None;
    }

    /// The messages an action is about: the open one from the reader, else the selected rows.
    fn targets(&self) -> Vec<Envelope> {
        let ws = self.ws();
        if self.focused_kind() == Kind::Reader {
            return ws.open.iter().cloned().collect();
        }
        let rows = self.rows();
        ws.selected_range().filter_map(|i| rows.get(i).cloned()).collect()
    }

    fn move_to_role(&mut self, role: FolderRole, label: &str) {
        let targets = self.targets();
        // Messages already there would make a command with no answer; leave them out.
        let ids: Vec<MessageId> = targets
            .iter()
            .filter(|e| {
                let account = self.mail.folder(&e.folder).map(|f| f.account.clone());
                let there = account.and_then(|a| self.mail.folder_with_role(&a, role).map(|f| f.id.clone()));
                there.as_ref() != Some(&e.folder)
            })
            .map(|e| e.id.clone())
            .collect();
        if ids.is_empty() {
            self.toast(if targets.is_empty() { "nothing selected" } else { "already there" });
            return;
        }
        self.close_reader_for(&ids);
        self.pending.push_back(PendingOp { label: label.into(), ids: ids.clone(), silent: false });
        self.send_op(Command::MoveToRole { ids, role });
    }

    fn toggle_flag(&mut self, flag: Flags, on: &str, off: &str) {
        let targets = self.targets();
        let Some(first) = targets.first() else { return self.toast("nothing selected") };
        let set = !first.flags.contains(flag);
        let ids: Vec<MessageId> = targets.iter().map(|e| e.id.clone()).collect();
        let (add, remove) = if set { (flag, Flags::empty()) } else { (Flags::empty(), flag) };
        self.pending.push_back(PendingOp { label: if set { on } else { off }.into(), ids: ids.clone(), silent: false });
        self.send_op(Command::SetFlags { ids, add, remove });
    }

    /// Send a journaled command. An inline engine that answers nothing (a no-op) must not leave
    /// its pending entry behind, or later answers would be credited to the wrong action.
    fn send_op(&mut self, cmd: Command) {
        let expected = self.pending.len().saturating_sub(1);
        let inline = matches!(self.link, crate::link::Link::Inline { .. });
        self.dispatch(cmd);
        if inline && self.pending.len() > expected { self.pending.truncate(expected); }
    }

    fn close_reader_for(&mut self, ids: &[MessageId]) {
        let ws = self.ws_mut();
        if ws.open.as_ref().is_some_and(|e| ids.contains(&e.id)) {
            ws.open = None;
            ws.panes.focus_kind(Kind::List);
        }
    }

    fn undo_last(&mut self) {
        match self.undo.pop() {
            Some(entry) => {
                let op = entry.op;
                self.undoing = Some(entry);
                self.dispatch(Command::Undo(op));
            }
            None => self.toast("nothing to undo"),
        }
    }

    fn switch_workspace(&mut self, slot: u8) {
        let Some(index) = self.workspaces.iter().position(|w| w.slot == slot) else {
            return self.toast(format!("no workspace {slot}"));
        };
        self.active = index;
        self.ensure_loaded();
    }

    /// Tabs are the open things of a workspace: its list, and the message in the reader.
    fn cycle_tab(&mut self) {
        let ws = self.ws_mut();
        if ws.open.is_none() { return; }
        let next = if ws.panes.focused_kind() == Kind::Reader { Kind::List } else { Kind::Reader };
        ws.panes.focus_kind(next);
    }

    fn split(&mut self, axis: Axis) {
        let area = self.frame.get().panes;
        let ws = self.ws_mut();
        let kind = if ws.panes.find(Kind::Reader).is_none() {
            Kind::Reader
        } else if ws.panes.find(Kind::List).is_none() {
            Kind::List
        } else {
            Kind::Empty
        };
        if let Err(e) = ws.panes.split(axis, kind, area) { self.error(e.message()); }
    }

    fn set_theme(&mut self, name: &str) {
        let theme = match name {
            "night" | "ravenglass-night" => Some(ravenglass_theme::Theme::night()),
            "frost" | "ravenglass-frost" => Some(ravenglass_theme::Theme::frost()),
            _ => None,
        };
        match theme {
            Some(theme) => {
                self.toast(format!("theme: {}", theme.name));
                self.theme = theme;
                self.fx.push(Fx::ThemeChanged);
            }
            None => self.error(format!("no theme named {name}")),
        }
    }

    fn go_to_mailbox(&mut self, name: &str) {
        let (account, folder_name) = match name.split_once('/') {
            Some((account, folder)) => (Some(account.to_owned()), folder),
            None => (None, name),
        };
        let current = match &self.ws().source {
            Source::Mailbox { account, .. } => Some(account.clone()),
            Source::Search(_) => self.accounts().into_iter().next(),
        };
        let account = match account {
            Some(wanted) => self.mail.folders.keys().find(|a| a.0.eq_ignore_ascii_case(&wanted)).cloned(),
            None => current,
        };
        let found = account.and_then(|a| self.mail.folder_named(&a, folder_name).map(|f| (a, f.id.clone(), f.name.clone())));
        let Some((account, folder, name)) = found else { return self.error(format!("no mailbox named {name}")) };
        let ws = self.ws_mut();
        ws.source = Source::Mailbox { account, name };
        ws.folder = Some(folder.clone());
        ws.search = None;
        ws.sel = 0;
        ws.anchor = None;
        ws.open = None;
        ws.panes.focus_kind(Kind::List);
        self.dispatch(Command::LoadRange { folder, offset: 0, limit: PAGE });
    }

    // ── search ─────────────────────────────────────────────────────────────────────────────

    /// Show results for `query` in the active list. An empty query goes back to the mailbox.
    pub fn search(&mut self, query: String) {
        if query.trim().is_empty() { return self.clear_search(); }
        let ws = self.ws_mut();
        let results = ws.search.take().map(|s| s.results).unwrap_or_default();
        ws.search = Some(Search { query: query.clone(), results, pending: true });
        ws.sel = 0;
        ws.anchor = None;
        self.dispatch(Command::Search { query, account: None, limit: PAGE });
    }

    /// Back to what the workspace shows by itself: its mailbox, or its saved search.
    pub fn clear_search(&mut self) {
        let saved = match &self.ws().source { Source::Search(q) => Some(q.clone()), Source::Mailbox { .. } => None };
        match saved {
            Some(query) if self.ws().search.as_ref().is_some_and(|s| s.query == query) => {}
            Some(query) => self.search(query),
            None => {
                let ws = self.ws_mut();
                ws.search = None;
                ws.sel = 0;
                ws.anchor = None;
            }
        }
    }

    /// True while the mode machine is in a mode with a text line (for the caret).
    pub fn typing(&self) -> bool { matches!(self.mode, Mode::Search { .. } | Mode::Command { .. } | Mode::Palette { .. }) }
}

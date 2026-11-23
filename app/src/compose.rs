//! Drafts on the app's side (plan §4 "Compose"): each open draft is a [`Composer`] holding the
//! `mail_core::Draft` the engine keeps and one Rune `TextEdit` per field. Every edit goes to the
//! engine as `EditDraft`; the engine saves it and debounces the server copy. Sending hands the
//! draft to the engine's outbox, whose states come back as `OutboxState` and drive the undo
//! countdown here.

use crate::keys::{K, KeyPress};
use crate::link::Link;
use crate::modes::Mode;
use crate::panes::{Axis, Dir, Kind, PaneId};
use crate::state::{AppState, Fx, Source};
use mail_core::machine::outbox;
use mail_core::{AccountId, Address, Draft, DraftId};
use ravenglass_engine::{Command, DraftSource, OutboxId, ReplyKind};
use rune::TextEdit;
use std::collections::BTreeMap;

/// Suggestions asked for per keystroke in an address field.
const COMPLETIONS: u32 = 6;
/// How often the countdown is looked at again, in seconds.
const COUNTDOWN_STEP: f64 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field { To, Cc, Subject, Body }

impl Field {
    pub const ALL: [Field; 4] = [Field::To, Field::Cc, Field::Subject, Field::Body];

    /// The field `tab` (or `shift+tab`) goes to; the ends wrap.
    pub fn next(self, back: bool) -> Field {
        let at = Field::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Field::ALL[(at + if back { Field::ALL.len() - 1 } else { 1 }) % Field::ALL.len()]
    }

    pub fn label(self) -> &'static str {
        match self { Field::To => "To", Field::Cc => "Cc", Field::Subject => "Subject", Field::Body => "Body" }
    }

    /// The element key of the field's editor.
    pub fn key(self) -> &'static str {
        match self { Field::To => "compose-to", Field::Cc => "compose-cc", Field::Subject => "compose-subject", Field::Body => "compose-body" }
    }

    pub fn takes_addresses(self) -> bool { matches!(self, Field::To | Field::Cc) }
}

/// Address suggestions under the field being typed in.
#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub field: Field,
    pub prefix: String,
    pub addresses: Vec<Address>,
    pub selected: usize,
}

/// One open draft.
pub struct Composer {
    /// What the engine has, as of the last edit.
    pub draft: Draft,
    pub to: TextEdit,
    pub cc: TextEdit,
    pub subject: TextEdit,
    pub body: TextEdit,
    /// The field the keyboard is in while composing.
    pub field: Field,
    /// Edits sent to the engine whose `DraftSaved` has not come back.
    pub unsaved: u32,
    /// The account's Drafts folder has this version too.
    pub on_server: bool,
    /// Set once the draft is sent: its place in the outbox and how far it got.
    pub outbox: Option<(OutboxId, outbox::State)>,
    pub completion: Option<Completion>,
}

impl Composer {
    fn new(draft: Draft) -> Composer {
        let edit = |text: String, caret_at_start: bool| {
            let mut e = TextEdit::new(text);
            if caret_at_start { e.set_caret(0); }
            e
        };
        // An answer starts above the quoted text; a new message starts with its recipients.
        let answer = draft.in_reply_to.is_some();
        Composer {
            to: edit(format_addresses(&draft.to), false),
            cc: edit(format_addresses(&draft.cc), false),
            subject: edit(draft.subject.clone(), false),
            body: edit(draft.body.clone(), true),
            field: if answer && !draft.to.is_empty() { Field::Body } else { Field::To },
            draft,
            unsaved: 0,
            on_server: false,
            outbox: None,
            completion: None,
        }
    }

    pub fn edit(&self, field: Field) -> &TextEdit {
        match field { Field::To => &self.to, Field::Cc => &self.cc, Field::Subject => &self.subject, Field::Body => &self.body }
    }

    pub fn edit_mut(&mut self, field: Field) -> &mut TextEdit {
        match field { Field::To => &mut self.to, Field::Cc => &mut self.cc, Field::Subject => &mut self.subject, Field::Body => &mut self.body }
    }

    /// The pane title: `reply ▸ Odin Allfather`, `forward`, `new message`.
    pub fn title(&self) -> String {
        let forward = self.draft.subject.trim_start().to_ascii_lowercase().starts_with("fwd:");
        match (&self.draft.in_reply_to, self.draft.to.first()) {
            (Some(_), Some(first)) if self.draft.recipients().len() > 1 => format!("reply ▸ {} +{}", first.display(), self.draft.recipients().len() - 1),
            (Some(_), Some(first)) => format!("reply ▸ {}", first.display()),
            (Some(_), None) => "reply".into(),
            (None, _) if forward => "forward".into(),
            (None, _) => "new message".into(),
        }
    }

    /// The title-bar tab: `Draft: Gnomon sync`.
    pub fn tab_title(&self) -> String {
        let subject = self.draft.subject.trim();
        format!("Draft: {}", if subject.is_empty() { "new message" } else { subject })
    }

    /// Editable: not handed to the outbox.
    pub fn editing(&self) -> bool { self.outbox.is_none() }

    /// Copy the fields into the draft. True when that changed it.
    fn sync(&mut self) -> bool {
        let before = (self.draft.to.clone(), self.draft.cc.clone(), self.draft.subject.clone(), self.draft.body.clone());
        self.draft.to = parse_addresses(&self.to.text);
        self.draft.cc = parse_addresses(&self.cc.text);
        self.draft.subject = self.subject.text.clone();
        self.draft.body = self.body.text.clone();
        before != (self.draft.to.clone(), self.draft.cc.clone(), self.draft.subject.clone(), self.draft.body.clone())
    }

    /// The address being typed in `field`: its byte range in the field's text and its text.
    fn token(&self, field: Field) -> (usize, usize, String) {
        let edit = self.edit(field);
        let caret = edit.range().1;
        let start = edit.text[..caret].rfind([',', ';']).map_or(0, |i| i + 1);
        (start, caret, edit.text[start..caret].trim().to_owned())
    }
}

/// `Name <email>, email` for an address field.
pub fn format_addresses(list: &[Address]) -> String {
    list.iter().map(format_address).collect::<Vec<_>>().join(", ")
}

fn format_address(a: &Address) -> String {
    match &a.name { Some(name) if !name.is_empty() => format!("{name} <{}>", a.email), _ => a.email.clone() }
}

/// The complete addresses in an address field; a token still being typed is left out.
pub fn parse_addresses(text: &str) -> Vec<Address> {
    text.split([',', ';']).filter_map(|token| {
        let token = token.trim();
        let (name, email) = match (token.find('<'), token.rfind('>')) {
            (Some(open), Some(close)) if open < close => (token[..open].trim().trim_matches('"'), token[open + 1..close].trim()),
            _ => ("", token),
        };
        let valid = email.split_once('@').is_some_and(|(local, host)| !local.is_empty() && !host.is_empty()) && !email.contains(char::is_whitespace);
        valid.then(|| Address { name: (!name.is_empty()).then(|| name.to_owned()), email: email.to_owned() })
    }).collect()
}

/// A compose pane that took another pane's place, to give it back when the drafts are gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Replaced { workspace: usize, pane: PaneId, kind: Kind }

/// Every open draft, and the one the compose pane shows.
#[derive(Default)]
pub struct Compose {
    pub drafts: Vec<Composer>,
    /// Index into `drafts`.
    pub active: usize,
    /// A `StartDraft` in flight: show its draft when it arrives.
    opening: Option<DraftId>,
    /// Outbox states that arrived before the `DraftClosed` that names their draft.
    early: BTreeMap<OutboxId, outbox::State>,
    replaced: Vec<Replaced>,
    /// Makes draft ids unique within one second.
    counter: u64,
    /// The app clock at the last tick, and the seconds the countdown showed then.
    last_clock: f64,
    shown_secs: Option<u64>,
    /// Stands in for a field when no draft is open.
    scratch: TextEdit,
}

impl Compose {
    pub fn current(&self) -> Option<&Composer> { self.drafts.get(self.active) }
    pub fn current_mut(&mut self) -> Option<&mut Composer> { self.drafts.get_mut(self.active) }

    /// The editor of `field` in the draft on show (for `El::on_edit`).
    pub fn edit_mut(&mut self, field: Field) -> &mut TextEdit {
        match self.drafts.get_mut(self.active) {
            Some(c) => c.edit_mut(field),
            None => &mut self.scratch,
        }
    }

    fn find(&self, id: &DraftId) -> Option<usize> { self.drafts.iter().position(|c| &c.draft.id == id) }

    /// The draft waiting in the outbox, if any.
    fn outgoing(&self) -> Option<(usize, OutboxId, &outbox::State)> {
        self.drafts.iter().enumerate().find_map(|(i, c)| c.outbox.as_ref().map(|(id, state)| (i, *id, state)))
    }
}

impl AppState {
    /// The engine's clock: fixed and moved by hand inline, wall time on its own thread.
    pub fn engine_now(&self) -> f64 {
        match &self.link {
            Link::Inline { now, .. } => *now,
            _ => ravenglass_engine::runner::now(),
        }
    }

    // ── opening ────────────────────────────────────────────────────────────────────────────

    /// `c`: back to the draft on show, or a new message when there is none.
    pub fn compose_or_resume(&mut self) {
        match self.compose.current() {
            Some(c) if c.editing() => self.show_draft(self.compose.active, true),
            _ => self.compose_new(),
        }
    }

    /// `enter` on the compose pane.
    pub fn compose_resume(&mut self) {
        if self.compose.current().is_some_and(Composer::editing) { self.show_draft(self.compose.active, true); }
    }

    pub fn compose_new(&mut self) {
        let account = match &self.ws().source {
            Source::Mailbox { account, .. } => Some(account.clone()),
            Source::Search(_) => self.accounts().into_iter().next(),
        };
        match account {
            Some(account) => self.start_draft(account, DraftSource::Blank),
            None => self.compose_failed("no account to write from"),
        }
    }

    /// `r` / `R` / `f`: answer the open message, else the selected one.
    pub fn compose_answer(&mut self, kind: ReplyKind) {
        let Some(envelope) = self.targets().into_iter().next() else { return self.compose_failed("no message to answer") };
        let Some(account) = self.mail.folder(&envelope.folder).map(|f| f.account.clone()) else { return self.compose_failed("no account to write from") };
        // Answered from the list: show the message being answered above the draft.
        let shown = self.ws().open.as_ref().is_some_and(|open| open.id == envelope.id);
        if !shown && self.selected().is_some_and(|e| e.id == envelope.id) { self.open_selected(false); }
        self.start_draft(account, DraftSource::Answer { message: envelope.id, kind });
    }

    fn start_draft(&mut self, account: AccountId, source: DraftSource) {
        self.compose.counter += 1;
        let id = DraftId(format!("draft-{}-{}", self.wall.now(), self.compose.counter));
        self.compose.opening = Some(id.clone());
        self.dispatch(Command::StartDraft { id, account, source });
    }

    fn compose_failed(&mut self, why: &str) {
        self.compose.opening = None;
        if self.mode == Mode::Compose { self.mode = Mode::default(); }
        self.error(why);
        self.sync_compose_focus();
    }

    /// An engine error while a draft was being opened or sent.
    pub fn compose_error(&mut self, context: &str) {
        if context == "draft" && self.compose.opening.take().is_some() && self.mode == Mode::Compose {
            self.mode = Mode::default();
            self.sync_compose_focus();
        }
    }

    // ── engine events ──────────────────────────────────────────────────────────────────────

    /// A draft the engine opened: the one just asked for, or one kept from an earlier run.
    pub fn fold_draft(&mut self, draft: Draft) {
        let asked = self.compose.opening.as_ref() == Some(&draft.id);
        let index = match self.compose.find(&draft.id) {
            Some(index) => index,
            None => {
                self.compose.drafts.push(Composer::new(draft));
                self.compose.drafts.len() - 1
            }
        };
        if asked {
            self.compose.opening = None;
            self.show_draft(index, true);
        }
    }

    pub fn fold_draft_saved(&mut self, id: &DraftId, on_server: bool) {
        let Some(c) = self.compose.find(id).and_then(|i| self.compose.drafts.get_mut(i)) else { return };
        if on_server { c.on_server = c.unsaved == 0; } else { c.unsaved = c.unsaved.saturating_sub(1); }
    }

    /// The engine's session ended: the draft is in the outbox, or it was discarded.
    pub fn fold_draft_closed(&mut self, id: &DraftId, outbox: Option<OutboxId>) {
        let Some(index) = self.compose.find(id) else { return };
        match outbox {
            Some(outbox) => {
                let state = self.compose.early.remove(&outbox).unwrap_or(outbox::State::Sending { attempt: 1 });
                self.compose.drafts[index].outbox = Some((outbox, state.clone()));
                self.compose.drafts[index].completion = None;
                self.outbox_changed(index, state);
            }
            None => {
                self.remove_draft(index);
                self.toast("draft discarded");
            }
        }
    }

    pub fn fold_outbox(&mut self, id: OutboxId, state: outbox::State) {
        let index = self.compose.drafts.iter().position(|c| c.outbox.as_ref().is_some_and(|(o, _)| *o == id));
        match index {
            Some(index) => {
                self.compose.drafts[index].outbox = Some((id, state.clone()));
                self.outbox_changed(index, state);
            }
            None => { self.compose.early.insert(id, state); }
        }
    }

    fn outbox_changed(&mut self, index: usize, state: outbox::State) {
        match state {
            outbox::State::Queued { .. } => {
                // The countdown runs in the status line; the keys are NORMAL's again (`u`).
                if self.mode == Mode::Compose { self.mode = Mode::default(); }
                self.fx.push(Fx::WakeAt(self.clock + COUNTDOWN_STEP));
            }
            outbox::State::Sending { .. } => {}
            outbox::State::Sent => {
                self.remove_draft(index);
                self.toast("sent");
            }
            outbox::State::Failed { error, .. } => self.error(format!("send failed: {error} · :retry")),
            outbox::State::Cancelled => {
                // The engine closed its session when it queued the message; open it again.
                let c = &mut self.compose.drafts[index];
                c.outbox = None;
                c.unsaved += 1;
                let draft = c.draft.clone();
                self.dispatch(Command::EditDraft(Box::new(draft)));
                self.toast("send cancelled · the draft is back");
                self.show_draft(index, true);
            }
        }
        self.sync_compose_focus();
    }

    pub fn fold_addresses(&mut self, prefix: &str, addresses: Vec<Address>) {
        let Some(c) = self.compose.current_mut() else { return };
        let field = c.field;
        if !field.takes_addresses() || !c.editing() { return; }
        // Only the answer to what is typed right now, without addresses already in the field.
        let (_, _, typed) = c.token(field);
        let have = parse_addresses(&c.edit(field).text);
        let addresses: Vec<Address> = addresses.into_iter().filter(|a| !have.iter().any(|h| h.email.eq_ignore_ascii_case(&a.email))).collect();
        c.completion = (typed == prefix && !addresses.is_empty()).then(|| Completion { field, prefix: prefix.to_owned(), addresses, selected: 0 });
    }

    // ── the pane ───────────────────────────────────────────────────────────────────────────

    /// Put draft `index` in the compose pane of the active workspace and focus the pane; with
    /// `edit`, give its field the keyboard (COMPOSE mode).
    pub fn show_draft(&mut self, index: usize, edit: bool) {
        if index >= self.compose.drafts.len() { return; }
        self.compose.active = index;
        self.open_compose_pane();
        if edit && self.compose.drafts[index].editing() { self.mode = Mode::Compose; }
        self.sync_compose_focus();
    }

    /// The compose pane of the active workspace: below the reader when a message is open
    /// (a narrow window shows the focused pane alone), else in place of the idle reader.
    fn open_compose_pane(&mut self) {
        let area = self.frame.get().panes;
        let workspace = self.active;
        let ws = self.ws_mut();
        if ws.panes.focus_kind(Kind::Compose) { return; }
        let idle = match (ws.panes.find(Kind::Empty), ws.panes.find(Kind::Reader)) {
            (Some(empty), _) => Some((empty, Kind::Empty)),
            (None, Some(reader)) if ws.open.is_none() => Some((reader, Kind::Reader)),
            _ => None,
        };
        if idle.is_none() {
            ws.panes.focus_kind(Kind::Reader);
            if ws.panes.split(Axis::Rows, Kind::Compose, area).is_ok() {
                // The reader keeps about three fifths of the height.
                let layout = ws.panes.layout(area);
                if let Some(rect) = layout.pane(ws.panes.focus()).map(|p| p.rect) {
                    let want = (area.h * 2 / 5).max(10);
                    let (dir, cells) = if rect.h > want { (Dir::Down, rect.h - want) } else { (Dir::Up, want - rect.h) };
                    if cells > 0 { let _ = ws.panes.resize(dir, cells, area); }
                }
                return;
            }
        }
        // No room to split, or a pane with nothing in it: the draft takes that pane.
        let (pane, kind) = idle.unwrap_or((ws.panes.focus(), ws.panes.focused_kind()));
        ws.panes.set_kind(pane, Kind::Compose);
        ws.panes.set_focus(pane);
        self.compose.replaced.push(Replaced { workspace, pane, kind });
    }

    /// The last draft is gone: compose panes give their room back.
    fn close_compose_panes(&mut self) {
        let replaced = std::mem::take(&mut self.compose.replaced);
        for (index, ws) in self.workspaces.iter_mut().enumerate() {
            while let Some(pane) = ws.panes.find(Kind::Compose) {
                match replaced.iter().find(|r| r.workspace == index && r.pane == pane) {
                    Some(r) => ws.panes.set_kind(pane, r.kind),
                    None => {
                        let back = ws.panes.focus();
                        ws.panes.set_focus(pane);
                        if ws.panes.close().is_err() { ws.panes.set_kind(pane, Kind::Empty); }
                        if back != pane { ws.panes.set_focus(back); }
                    }
                }
            }
            if ws.panes.focused_kind() == Kind::Empty { ws.panes.focus_kind(Kind::List); }
        }
    }

    fn remove_draft(&mut self, index: usize) {
        self.compose.drafts.remove(index);
        if self.compose.active > index || self.compose.active >= self.compose.drafts.len() {
            self.compose.active = self.compose.active.saturating_sub(1);
        }
        if self.compose.drafts.is_empty() { self.close_compose_panes(); }
        if self.mode == Mode::Compose { self.mode = Mode::default(); }
        self.sync_compose_focus();
    }

    // ── keys and edits ─────────────────────────────────────────────────────────────────────

    /// Make the keyboard follow the mode: in COMPOSE the draft's field has it, otherwise no
    /// field does. Rune keeps `TextEdit::focused` after that.
    pub fn sync_compose_focus(&mut self) {
        let composing = self.mode == Mode::Compose;
        let Some(c) = self.compose.drafts.get_mut(self.compose.active) else {
            if composing { self.mode = Mode::default(); }
            return;
        };
        if composing && c.editing() {
            let current = c.field;
            for field in Field::ALL {
                let edit = c.edit_mut(field);
                if edit.focused != (field == current) { edit.focused = field == current; }
            }
            return;
        }
        if composing { self.mode = Mode::default(); }
        c.completion = None;
        let mut held = false;
        for field in Field::ALL {
            let edit = c.edit_mut(field);
            held |= edit.focused;
            edit.focused = false;
        }
        if held { self.fx.push(Fx::Blur); }
    }

    /// `tab` / `shift+tab`.
    pub fn compose_field(&mut self, back: bool) {
        if let Some(c) = self.compose.current_mut() {
            c.field = c.field.next(back);
            c.completion = None;
        }
    }

    /// A key pressed in a focused field, before its editor sees it. True when it was taken.
    pub fn compose_input_key(&mut self, field: Field, press: KeyPress) -> bool {
        let Some(c) = self.compose.current_mut() else { return false };
        if c.field != field {
            c.field = field;
            c.completion = None;
        }
        // A click put the caret here: the mode follows the keyboard.
        if self.mode != Mode::Compose { self.mode = Mode::Compose; }
        self.completion_key(press) || self.mode_key(press)
    }

    /// The suggestion list's keys while it is open.
    fn completion_key(&mut self, press: KeyPress) -> bool {
        let Some(c) = self.compose.current_mut() else { return false };
        let Some(list) = &mut c.completion else { return false };
        let count = list.addresses.len();
        match press.key {
            K::Down => list.selected = (list.selected + 1) % count,
            K::Up => list.selected = (list.selected + count - 1) % count,
            K::Esc => c.completion = None,
            K::Tab | K::Enter if !press.shift => self.accept_completion(),
            _ => return false,
        }
        true
    }

    /// Put the selected suggestion in place of what was typed.
    fn accept_completion(&mut self) {
        let Some(c) = self.compose.current_mut() else { return };
        let Some(list) = c.completion.take() else { return };
        let Some(address) = list.addresses.get(list.selected) else { return };
        let (start, end, _) = c.token(list.field);
        let edit = c.edit_mut(list.field);
        let lead = if start == 0 { "" } else { " " };
        let head = format!("{}{lead}{}, ", &edit.text[..start], format_address(address));
        let tail = edit.text[end..].trim_start().to_owned();
        let caret = head.len();
        edit.replace_text(format!("{head}{tail}"));
        edit.set_caret(caret);
        self.compose_changed(list.field);
    }

    /// A field's text changed: the engine gets the draft, an address field asks for suggestions.
    pub fn compose_changed(&mut self, field: Field) {
        let Some(c) = self.compose.current_mut() else { return };
        if !c.editing() { return; }
        if c.sync() {
            c.unsaved += 1;
            c.on_server = false;
            let draft = c.draft.clone();
            self.dispatch(Command::EditDraft(Box::new(draft)));
        }
        if !field.takes_addresses() { return; }
        let Some(c) = self.compose.current_mut() else { return };
        let (_, _, prefix) = c.token(field);
        if prefix.is_empty() {
            c.completion = None;
        } else {
            self.dispatch(Command::CompleteAddress { prefix, limit: COMPLETIONS });
        }
    }

    /// A key that reached the window in COMPOSE mode: no field has the keyboard (a click
    /// elsewhere took it), so type it into the draft's field and take the keyboard back.
    pub fn compose_window_key(&mut self, press: KeyPress) -> bool {
        if self.mode != Mode::Compose { return false; }
        let Some(c) = self.compose.current_mut() else { return false };
        let field = c.field;
        let text = match (press.key, press.typed()) {
            (_, Some(typed)) => typed.to_string(),
            (K::Enter, _) if field == Field::Body => "\n".to_owned(),
            _ => return false,
        };
        c.edit_mut(field).insert(&text);
        self.compose_changed(field);
        self.sync_compose_focus();
        true
    }

    // ── sending ────────────────────────────────────────────────────────────────────────────

    /// `:send` / `mod+shift+d`.
    pub fn send_draft(&mut self) {
        let Some(c) = self.compose.current_mut() else { return self.toast("no draft to send") };
        if !c.editing() { return; }
        let field = c.field;
        self.compose_changed(field);
        let Some(c) = self.compose.current_mut() else { return };
        if c.draft.recipients().is_empty() {
            c.field = Field::To;
            self.error("add a recipient first");
            return self.sync_compose_focus();
        }
        let id = c.draft.id.clone();
        self.dispatch(Command::SendDraft(id));
    }

    /// `:discard`.
    pub fn discard_draft(&mut self) {
        match self.compose.current() {
            Some(c) if c.editing() => { let id = c.draft.id.clone(); self.dispatch(Command::DiscardDraft(id)); }
            Some(_) => self.toast("already sending · u undo"),
            None => self.toast("no draft to discard"),
        }
    }

    /// `u` while a message waits in the outbox. True when there was one to take back.
    pub fn cancel_send(&mut self) -> bool {
        let Some((_, id, outbox::State::Queued { .. })) = self.compose.outgoing() else { return false };
        self.dispatch(Command::CancelSend(id));
        true
    }

    /// `:retry` after a failed send.
    pub fn retry_send(&mut self) {
        match self.compose.outgoing() {
            Some((_, id, outbox::State::Failed { .. })) => self.dispatch(Command::RetrySend(id)),
            _ => self.toast("nothing to retry"),
        }
    }

    /// What the status line says about the message on its way out.
    pub fn sending_label(&self) -> Option<String> {
        let (_, _, state) = self.compose.outgoing()?;
        Some(match state {
            outbox::State::Queued { undo_until } => format!("sending in {}s · u undo", (undo_until - self.engine_now()).ceil().max(0.0) as u64),
            outbox::State::Sending { .. } => "sending…".into(),
            outbox::State::Failed { .. } => "send failed · :retry".into(),
            outbox::State::Sent | outbox::State::Cancelled => return None,
        })
    }

    /// Timers. The engine's own thread delivers its timers itself; the inline engine of
    /// headless runs has no thread, so while a message waits its clock follows the app's and
    /// the due `Tick` is sent from here. True when the countdown moved.
    pub fn compose_tick(&mut self, clock: f64) -> bool {
        let elapsed = (clock - self.compose.last_clock).max(0.0);
        self.compose.last_clock = clock;
        let Some((_, _, state)) = self.compose.outgoing() else {
            return self.compose.shown_secs.take().is_some();
        };
        if let outbox::State::Queued { undo_until } = *state {
            self.fx.push(Fx::WakeAt(clock + COUNTDOWN_STEP));
            if let Link::Inline { now, .. } = &mut self.link {
                *now += elapsed;
                if *now >= undo_until { self.dispatch(Command::Tick); }
            }
        }
        let secs = match self.compose.outgoing() {
            Some((_, _, outbox::State::Queued { undo_until })) => Some((undo_until - self.engine_now()).ceil().max(0.0) as u64),
            _ => None,
        };
        let moved = secs != self.compose.shown_secs;
        self.compose.shown_secs = secs;
        moved
    }

    /// Move the inline engine's clock on by `secs` and deliver its due timers: how scripts and
    /// tests let the undo window pass. Does nothing for the threaded engine, which keeps time.
    pub fn advance_engine(&mut self, secs: f64) {
        if let Link::Inline { now, .. } = &mut self.link {
            *now += secs.max(0.0);
            self.dispatch(Command::Tick);
        }
    }
}

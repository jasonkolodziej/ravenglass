//! Headless scenes (plan §5): named states for `--snap`, `--tree` and `--script`. Each starts
//! from the fully loaded inline state, so one frame shows data.

use crate::actions::Action;
use crate::modes::Mode;
use crate::panes::{Axis, Kind, PaneTree};
use crate::state::AppState;
use ravenglass_theme::Theme;

type Scene = (&'static str, fn(&mut AppState));

/// The triage view of the previews: the first message selected and open in the reader.
fn triage(s: &mut AppState) {
    // Prefer an unread reply-style message near the top, so the reader shows a quoted block.
    let pick = s.rows().iter().take(8).position(|e| e.is_unread() && crate::demo::is_person(&e.from.email));
    s.ws_mut().sel = pick.unwrap_or(0);
    s.open_selected(false);
}

/// Select the first message from `sender` in the list and open it.
fn open_from(s: &mut AppState, sender: &str) {
    if let Some(row) = s.rows().iter().position(|e| e.from.email.starts_with(sender)) {
        s.ws_mut().sel = row;
        s.open_selected(false);
    }
}

fn html_newsletter(s: &mut AppState) { open_from(s, "news@dioxus"); }

fn html_receipt(s: &mut AppState) { open_from(s, "billing@fastmail"); }

fn html_frost(s: &mut AppState) {
    frost(s);
    open_from(s, "news@dioxus");
}

fn frost(s: &mut AppState) {
    s.theme = Theme::frost();
    triage(s);
}

fn wallpaper(s: &mut AppState) {
    s.ws_mut().panes = PaneTree::single(Kind::Reader);
}

fn wallpaper_frost(s: &mut AppState) {
    s.theme = Theme::frost();
    wallpaper(s);
}

/// Run at `--size 800x700`: the layout collapses to the list alone.
fn narrow(_: &mut AppState) {}

/// The narrow layout after `enter`: the reader alone.
fn narrow_reader(s: &mut AppState) {
    s.perform(Action::Open);
}

fn split_panes(s: &mut AppState) {
    triage(s);
    s.ws_mut().panes.focus_kind(Kind::Reader);
    s.perform(Action::PaneSplit(Axis::Rows));
    s.perform(Action::PaneFocus(crate::panes::Dir::Up));
}

fn command_line(s: &mut AppState) {
    triage(s);
    s.mode = Mode::Command { line: "theme frost".into() };
}

fn palette(s: &mut AppState) {
    triage(s);
    s.mode = Mode::Palette { query: "pane".into(), selected: 1 };
}

fn search_results(s: &mut AppState) {
    let query = "from:github is:unread";
    s.mode = Mode::Search { query: query.into() };
    s.search(query.into());
}

fn empty_inbox(s: &mut AppState) {
    s.perform(Action::Mailbox("Trash".into()));
}

fn visual(s: &mut AppState) {
    for key in ['j', 'v', 'j', 'j'] { s.key(crate::keys::KeyPress::char(key)); }
}

/// After archiving one message: the undo toast.
fn undo_toast(s: &mut AppState) {
    s.perform(Action::Archive);
}

fn reader_focus(s: &mut AppState) {
    s.perform(Action::Open);
}

fn workspace_gmail(s: &mut AppState) {
    s.perform(Action::Workspace(2));
}

/// Type `text` into the draft's focused field, as the keyboard would.
fn type_into_draft(s: &mut AppState, text: &str) {
    let Some(field) = s.compose.current().map(|c| c.field) else { return };
    s.compose.edit_mut(field).insert(text);
    s.compose_changed(field);
}

/// `c`: a new message, part written.
fn compose(s: &mut AppState) {
    s.perform(Action::Compose);
    type_into_draft(s, "Huginn <huginn@asgard.example>, ");
    s.compose_field(false);
    s.compose_field(false);
    type_into_draft(s, "Gnomon sync");
    s.compose_field(false);
    type_into_draft(s, "The build goes north on the morning ferry.\n\nSend word when the pass is clear.");
    s.sync_compose_focus();
}

/// The preview's reply: the open message answered in a pane below the reader.
fn compose_reply(s: &mut AppState) {
    triage(s);
    s.perform(Action::Reply);
    type_into_draft(s, "Thanks — sending the build north today. Tell Huginn\nto watch the ferry dock.");
}

fn compose_reply_frost(s: &mut AppState) {
    s.theme = Theme::frost();
    compose_reply(s);
}

/// Run at `--size 800x700`: the draft takes the whole pane area.
fn compose_narrow(s: &mut AppState) { compose_reply(s); }

/// A reply waiting out its undo window.
fn compose_sending(s: &mut AppState) {
    compose_reply(s);
    s.perform(Action::Send);
}

pub const SCENES: &[Scene] = &[
    ("default", triage),
    ("night", triage),
    ("frost", frost),
    ("wallpaper", wallpaper),
    ("wallpaper-frost", wallpaper_frost),
    ("narrow", narrow),
    ("narrow-reader", narrow_reader),
    ("split-panes", split_panes),
    ("command-line", command_line),
    ("palette", palette),
    ("search-results", search_results),
    ("empty-inbox", empty_inbox),
    ("visual", visual),
    ("undo-toast", undo_toast),
    ("reader-focus", reader_focus),
    ("workspace-gmail", workspace_gmail),
    ("html-newsletter", html_newsletter),
    ("html-receipt", html_receipt),
    ("html-frost", html_frost),
    ("compose", compose),
    ("compose-reply", compose_reply),
    ("compose-reply-frost", compose_reply_frost),
    ("compose-narrow", compose_narrow),
    ("compose-sending", compose_sending),
];

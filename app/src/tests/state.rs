//! State changes through the engine: the inline host the scenes use, and the threaded one the
//! window uses. Both fold the same events.

use crate::actions::Action;
use crate::keys::{K, KeyPress};
use crate::modes::Mode;
use crate::panes::Kind;
use crate::state::*;
use mail_core::Flags;
use ravenglass_engine::ReaderView;
use std::time::{Duration, Instant};

fn app() -> AppState { AppState::demo_inline() }

fn keys(s: &mut AppState, text: &str) {
    for c in text.chars() { s.key(KeyPress::char(c)); }
}

fn enter(s: &mut AppState) { s.key(KeyPress::named(K::Enter)); }
fn esc(s: &mut AppState) { s.key(KeyPress::named(K::Esc)); }

fn toast(s: &AppState) -> &str { s.toast.as_ref().map_or("", |t| t.text.as_str()) }

#[test]
fn the_inline_engine_loads_both_accounts_before_the_first_frame() {
    let s = app();
    assert_eq!(s.mail.folders.len(), 2);
    let folder = s.folder().expect("the inbox resolved");
    assert_eq!(folder.name, "Inbox");
    assert_eq!(s.rows().len() as u32, folder.total);
    assert!(s.rows().len() > 50);
    assert!(s.rows().windows(2).all(|w| w[0].date >= w[1].date), "newest first");
    assert!(s.toast.is_none(), "{:?}", s.toast);
    assert_eq!(crate::view::workspaces::sync_text(&s), "synced 16:44 · IDLE");
}

#[test]
fn j_and_k_move_the_selection_and_stop_at_the_ends() {
    let mut s = app();
    keys(&mut s, "jjj");
    assert_eq!(s.ws().sel, 3);
    keys(&mut s, "kkkkk");
    assert_eq!(s.ws().sel, 0);
    keys(&mut s, "G");
    assert_eq!(s.ws().sel, s.rows().len() - 1);
    keys(&mut s, "j");
    assert_eq!(s.ws().sel, s.rows().len() - 1);
    keys(&mut s, "gg");
    assert_eq!(s.ws().sel, 0);
    assert!(s.fx.iter().any(|fx| matches!(fx, Fx::Reveal(_))), "moving asks the list to follow");
}

#[test]
fn archive_goes_through_the_engine_and_undo_brings_it_back() {
    let mut s = app();
    keys(&mut s, "j");
    let before: Vec<_> = s.rows().iter().map(|e| e.id.clone()).collect();
    let target = before[1].clone();
    keys(&mut s, "e");
    assert_eq!(toast(&s), "archived 1 · u undo");
    assert_eq!(s.rows().len(), before.len() - 1);
    assert!(s.rows().iter().all(|e| e.id != target));
    assert_eq!(s.ws().sel, 1, "the next message slides under the cursor");
    assert_eq!(s.undo.len(), 1);
    assert!(s.pending.is_empty());
    let archive = s.mail.folder_with_role(&"iCloud".into(), mail_core::FolderRole::Archive).unwrap().id.clone();
    assert!(s.mail.lists.get(&archive).is_none_or(|l| l.iter().any(|e| e.id == target)), "a loaded Archive list shows it");

    keys(&mut s, "u");
    assert_eq!(toast(&s), "undone: archived 1");
    assert_eq!(s.rows().iter().map(|e| e.id.clone()).collect::<Vec<_>>(), before);
    assert_eq!(s.selected().map(|e| &e.id), Some(&target), "the restored message is selected");
    assert!(s.undo.is_empty());
    keys(&mut s, "u");
    assert_eq!(toast(&s), "nothing to undo");
}

#[test]
fn delete_moves_to_trash_and_counts_follow() {
    let mut s = app();
    let total = s.folder().unwrap().total;
    let target = s.selected().unwrap().id.clone();
    keys(&mut s, "#");
    assert_eq!(toast(&s), "deleted 1 · u undo");
    assert_eq!(s.folder().unwrap().total, total - 1);
    s.perform(Action::Mailbox("Trash".into()));
    assert_eq!(s.rows().iter().map(|e| &e.id).collect::<Vec<_>>(), [&target]);
    keys(&mut s, "#");
    assert_eq!(toast(&s), "already there", "a no-op is not sent to the engine");
    assert!(s.pending.is_empty());
}

#[test]
fn read_and_flag_toggle_and_undo() {
    let mut s = app();
    let was_unread = s.selected().unwrap().is_unread();
    keys(&mut s, "U");
    assert_eq!(s.selected().unwrap().is_unread(), !was_unread);
    assert_eq!(toast(&s), if was_unread { "marked read 1 · u undo" } else { "marked unread 1 · u undo" });
    keys(&mut s, "s");
    assert!(s.selected().unwrap().flags.contains(Flags::FLAGGED));
    assert_eq!(toast(&s), "flagged 1 · u undo");
    keys(&mut s, "uu");
    assert!(!s.selected().unwrap().flags.contains(Flags::FLAGGED));
    assert_eq!(s.selected().unwrap().is_unread(), was_unread);
}

#[test]
fn visual_mode_acts_on_the_whole_range() {
    let mut s = app();
    let len = s.rows().len();
    keys(&mut s, "jvjj");
    assert_eq!(s.mode.label(), "VISUAL");
    assert_eq!(s.ws().selected_range(), 1..=3);
    keys(&mut s, "e");
    assert_eq!(toast(&s), "archived 3 · u undo");
    assert_eq!(s.rows().len(), len - 3);
    assert_eq!(s.mode, Mode::default());
    assert_eq!(s.ws().anchor, None);
    keys(&mut s, "u");
    assert_eq!(s.rows().len(), len);
}

#[test]
fn enter_opens_marks_read_and_focuses_the_reader_and_escape_goes_back() {
    let mut s = app();
    let at = s.rows().iter().position(|e| e.is_unread()).expect("the seed has unread mail");
    s.ws_mut().sel = at;
    let id = s.selected().unwrap().id.clone();
    enter(&mut s);
    assert_eq!(s.focused_kind(), Kind::Reader);
    assert_eq!(s.ws().open.as_ref().map(|e| &e.id), Some(&id));
    let body = s.body().expect("the body arrived");
    assert!(!body.text.lines.is_empty());
    assert!(!s.rows()[at].is_unread(), "opening marks it read");
    assert!(s.toast.is_none() && s.undo.is_empty(), "marking read on open is not an undoable action");
    // In the reader j scrolls rather than moving the selection.
    keys(&mut s, "j");
    assert_eq!(s.ws().sel, at);
    esc(&mut s);
    assert_eq!(s.focused_kind(), Kind::List);
    assert!(s.ws().open.is_some(), "the reader keeps the message");
    keys(&mut s, "gt");
    assert_eq!(s.focused_kind(), Kind::Reader, "gt cycles the tabs: list, message");
    keys(&mut s, "gT");
    assert_eq!(s.focused_kind(), Kind::List);
}

#[test]
fn archiving_the_open_message_from_the_reader_closes_it() {
    let mut s = app();
    let id = s.selected().unwrap().id.clone();
    enter(&mut s);
    keys(&mut s, "e");
    assert!(s.ws().open.is_none());
    assert_eq!(s.focused_kind(), Kind::List);
    assert!(s.rows().iter().all(|e| e.id != id));
}

#[test]
fn t_flips_the_reader_view_flag() {
    let mut s = app();
    assert_eq!(s.reader_view, ReaderView::Html, "HTML is the product default");
    keys(&mut s, "t");
    assert_eq!(s.reader_view, ReaderView::Text);
    assert_eq!(toast(&s), "reader view: text");
    keys(&mut s, "t");
    assert_eq!(s.reader_view, ReaderView::Html);
}

#[test]
fn search_streams_results_into_the_list_and_escape_restores_the_mailbox() {
    let mut s = app();
    let inbox = s.rows().len();
    keys(&mut s, "/from:github");
    assert_eq!(s.mode.label(), "SEARCH");
    let search = s.ws().search.as_ref().expect("a search is on show");
    assert_eq!((search.query.as_str(), search.pending), ("from:github", false));
    assert!(!s.rows().is_empty() && s.rows().iter().all(|e| e.from.email.contains("github")));
    enter(&mut s);
    assert_eq!(s.mode, Mode::default());
    assert!(s.ws().search.is_some(), "enter keeps the results");
    esc(&mut s);
    assert!(s.ws().search.is_none());
    assert_eq!(s.rows().len(), inbox);
    keys(&mut s, "/zz");
    esc(&mut s);
    assert!(s.ws().search.is_none(), "escape in SEARCH drops the query");
}

#[test]
fn the_command_line_runs_the_same_actions() {
    let mut s = app();
    let len = s.rows().len();
    keys(&mut s, ":archive");
    enter(&mut s);
    assert_eq!(toast(&s), "archived 1 · u undo");
    assert_eq!(s.rows().len(), len - 1);
    keys(&mut s, ":theme frost");
    enter(&mut s);
    assert_eq!(s.theme.id, "ravenglass-frost");
    assert!(s.fx.contains(&Fx::ThemeChanged));
    keys(&mut s, ":search is:unread");
    enter(&mut s);
    assert!(!s.rows().is_empty() && s.rows().iter().all(|e| e.is_unread()));
    keys(&mut s, ":mailbox receipts");
    enter(&mut s);
    assert_eq!(s.folder().map(|f| f.name.as_str()), Some("Receipts"));
    assert!(s.ws().search.is_none());
    keys(&mut s, ":mailbox Gmail/Inbox");
    enter(&mut s);
    assert_eq!(s.folder().map(|f| (f.account.0.as_str(), f.name.as_str())), Some(("Gmail", "Inbox")));
    keys(&mut s, ":nonsense");
    enter(&mut s);
    assert!(s.toast.as_ref().is_some_and(|t| t.error && t.text.contains("unknown command")));
    keys(&mut s, ":theme mauve");
    enter(&mut s);
    assert_eq!(toast(&s), "no theme named mauve");
}

#[test]
fn the_palette_runs_the_selected_action() {
    let mut s = app();
    s.perform(Action::Palette);
    assert!(matches!(s.mode, Mode::Palette { .. }));
    keys(&mut s, "theme frost");
    enter(&mut s);
    assert_eq!(s.theme.id, "ravenglass-frost");
    assert_eq!(s.mode, Mode::default());
}

#[test]
fn digits_switch_workspaces_each_with_its_own_selection_and_layout() {
    let mut s = app();
    keys(&mut s, "jj");
    keys(&mut s, "2");
    assert_eq!(s.ws().name, "Gmail");
    assert_eq!(s.folder().map(|f| f.account.0.as_str()), Some("Gmail"));
    assert!(!s.rows().is_empty());
    assert_eq!(s.ws().sel, 0);
    s.perform(Action::PaneClose);
    assert_eq!(s.ws().panes.leaves().len(), 1);
    keys(&mut s, "6");
    assert!(s.rows().iter().all(|e| e.from.email.contains("github")), "a saved search workspace");
    esc(&mut s);
    assert!(s.ws().search.is_some(), "its search is what it is, escape does not clear it");
    keys(&mut s, "1");
    assert_eq!((s.ws().name.as_str(), s.ws().sel), ("iCloud", 2));
    assert_eq!(s.ws().panes.leaves().len(), 2, "layouts are per workspace");
    keys(&mut s, "4");
    assert_eq!(toast(&s), "no workspace 4");
}

#[test]
fn pane_keys_split_focus_resize_and_close() {
    let mut s = app();
    let ctrl_w = KeyPress { ctrl: true, cmd: !cfg!(target_os = "macos"), ..KeyPress::char('w') };
    let chord = |s: &mut AppState, c: char| {
        s.key(ctrl_w);
        s.key(KeyPress::char(c));
    };
    chord(&mut s, 'l');
    assert_eq!(s.focused_kind(), Kind::Reader);
    chord(&mut s, 's');
    assert_eq!(s.ws().panes.leaves().len(), 3);
    assert_eq!(s.focused_kind(), Kind::Empty);
    chord(&mut s, 'k');
    assert_eq!(s.focused_kind(), Kind::Reader);
    chord(&mut s, 'h');
    assert_eq!(s.focused_kind(), Kind::List);
    let area = s.frame.get().panes;
    let before = s.ws().panes.layout(area).panes[0].rect.w;
    chord(&mut s, '>');
    assert_eq!(s.ws().panes.layout(area).panes[0].rect.w, before + 2);
    chord(&mut s, 'q');
    assert_eq!(s.ws().panes.leaves().len(), 2);
    chord(&mut s, 'q');
    chord(&mut s, 'q');
    assert_eq!(toast(&s), "can't close the last pane");
}

#[test]
fn toasts_expire_on_the_app_clock() {
    let mut s = app();
    s.clock = 10.0;
    keys(&mut s, "e");
    assert!(s.fx.contains(&Fx::WakeAt(16.0)));
    assert!(!s.tick(15.9));
    assert!(s.toast.is_some());
    assert!(s.tick(16.0), "the due time is a visible change");
    assert!(s.toast.is_none());
    assert!(!s.tick(17.0), "an idle tick changes nothing");
}

#[test]
fn the_body_cache_is_bounded() {
    let mut s = app();
    for i in 0..BodyCache::MAX_ENTRIES + 6 {
        s.ws_mut().sel = i;
        s.ws_mut().panes.focus_kind(Kind::List);
        s.open_selected(false);
    }
    assert_eq!(s.mail.bodies.len(), BodyCache::MAX_ENTRIES);
    assert!(s.body().is_some(), "the newest body is kept");
}

/// Pump until both accounts are idle and nothing more has arrived for a moment.
fn settle(s: &mut AppState) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut quiet_since = Instant::now();
    loop {
        let now = Instant::now();
        if s.pump() { quiet_since = now; }
        let idle = s.mail.sync.len() == 2 && s.mail.sync.values().all(|st| *st == mail_core::machine::account_sync::State::Idle);
        if (idle && s.pending.is_empty() && now.duration_since(quiet_since) > Duration::from_millis(300)) || now > deadline { return; }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_engine_thread_delivers_the_same_state_through_poll() {
    let mut s = AppState::windowed();
    assert!(s.rows().is_empty() && s.link.is_off());
    assert_eq!(crate::view::workspaces::sync_text(&s), "offline");
    s.waker = Some(std::sync::Arc::new(|| {}));
    s.start_engine();
    assert_eq!(s.boot, Boot::Started);
    settle(&mut s);
    let inline = app();
    for (threaded, inline_ws) in s.workspaces.iter().zip(&inline.workspaces) {
        let ids = |s: &AppState, w: &Workspace| s.rows_of(w).iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&s, threaded), ids(&inline, inline_ws), "workspace {}", threaded.name);
    }
    // An action answers through the same channel.
    s.key(KeyPress::char('e'));
    assert_eq!(s.pending.len(), 1);
    settle(&mut s);
    assert_eq!(toast(&s), "archived 1 · u undo");
    assert!(s.pending.is_empty());
}

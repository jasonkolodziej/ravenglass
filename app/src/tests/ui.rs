//! Headless runs of the real view through Rune's `Runtime`: what is on screen, as `--tree`
//! reports it, before and after keys.

use crate::state::AppState;
use crate::view::{self, Paint};
use rune::runtime::ElementInfo;
use rune::text::TextSystem;
use rune::{Env, Key, KeyEvent, Mods, Runtime};

const FRAME: f64 = 1.0 / 60.0;

pub(super) fn runtime_with(state: AppState, size: (f32, f32)) -> Runtime<AppState> {
    let text = TextSystem::new(crate::FONTS, crate::FAMILIES);
    let mut rt = Runtime::with_text(state, view::root, size, Env { system_dark: true, scale: 2.0 }, text);
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt.frame(FRAME);
    rt
}

pub(super) fn runtime() -> Runtime<AppState> { runtime_with(AppState::demo_inline(), (1400.0, 880.0)) }

pub(super) fn scene(name: &str, size: (f32, f32)) -> Runtime<AppState> {
    let mut state = AppState::demo_inline();
    let (_, setup) = crate::scenes::SCENES.iter().find(|(n, _)| *n == name).expect("a known scene");
    setup(&mut state);
    runtime_with(state, size)
}

pub(super) fn settle(rt: &mut Runtime<AppState>) {
    for _ in 0..90 { rt.frame(FRAME); }
}

pub(super) fn press(rt: &mut Runtime<AppState>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(FRAME);
}

pub(super) fn type_keys(rt: &mut Runtime<AppState>, text: &str) {
    for c in text.chars() { press(rt, Key::Text(c.to_string())); }
}

pub(super) fn keyed<'a>(els: &'a [ElementInfo], key: &str) -> Option<&'a ElementInfo> { els.iter().find(|e| e.key.as_deref() == Some(key)) }

/// The text drawn inside the element keyed `key`.
pub(super) fn text_in(els: &[ElementInfo], key: &str) -> String {
    let Some(at) = els.iter().position(|e| e.key.as_deref() == Some(key)) else { return String::new() };
    let inside = |mut i: usize| loop {
        if i == at { return true; }
        match els[i].parent { Some(p) => i = p, None => return false }
    };
    els.iter().enumerate().filter(|(i, _)| inside(*i)).filter_map(|(_, e)| e.text.clone()).collect::<Vec<_>>().join(" ").trim().to_owned()
}

/// Keys of the list rows drawn with the selection fill.
fn selected_rows(rt: &Runtime<AppState>) -> Vec<String> {
    let selection = Paint::of(&rt.state.theme, &rt.state.mode).selection;
    rt.elements().iter().filter(|e| e.role == "listitem" && e.fill == selection).filter_map(|e| e.key.clone()).collect()
}

fn selected_key(s: &AppState) -> String {
    let pane = s.ws().panes.find(crate::panes::Kind::List).expect("a list pane");
    AppState::row_key(pane, &s.selected().expect("a selected message").id)
}

#[test]
fn the_list_has_role_list_and_the_selected_row_matches_state() {
    let rt = runtime();
    let els = rt.elements();
    let list = els.iter().find(|e| e.role == "list").expect("a list");
    assert_eq!(list.key.as_deref(), Some("list:1"));
    assert_eq!(list.label.as_deref(), Some("Inbox"));
    assert!(els.iter().filter(|e| e.role == "listitem").count() >= 15, "rows are mounted");
    assert_eq!(selected_rows(&rt), [selected_key(&rt.state)]);
}

#[test]
fn the_mode_block_shows_the_mode() {
    let mut rt = runtime();
    let status = |rt: &Runtime<AppState>| {
        let els = rt.elements();
        (keyed(&els, "status").and_then(|e| e.label.clone()).unwrap_or_default(), text_in(&els, "status"))
    };
    let (label, text) = status(&rt);
    assert_eq!(label, "NORMAL mode");
    assert!(text.contains("NORMAL") && text.contains("iCloud/Inbox") && text.contains("mail"), "{text}");
    press(&mut rt, Key::Text(":".into()));
    type_keys(&mut rt, "theme");
    let (label, text) = status(&rt);
    assert_eq!(label, "COMMAND mode");
    assert!(text.contains("COMMAND") && text.contains(":theme"), "{text}");
    press(&mut rt, Key::Escape);
    press(&mut rt, Key::Text("/".into()));
    assert_eq!(status(&rt).0, "SEARCH mode");
    press(&mut rt, Key::Escape);
    press(&mut rt, Key::Text("v".into()));
    assert_eq!(status(&rt).0, "VISUAL mode");
}

#[test]
fn j_and_k_move_the_selection_on_screen() {
    let mut rt = runtime();
    let first = selected_key(&rt.state);
    type_keys(&mut rt, "j");
    settle(&mut rt);
    let second = selected_key(&rt.state);
    assert_ne!(first, second);
    assert_eq!(selected_rows(&rt), [second]);
    type_keys(&mut rt, "jj");
    settle(&mut rt);
    assert_eq!(rt.state.ws().sel, 3);
    assert_eq!(selected_rows(&rt), [selected_key(&rt.state)]);
    type_keys(&mut rt, "kkk");
    settle(&mut rt);
    assert_eq!(selected_rows(&rt), [first]);
}

#[test]
fn archive_removes_the_row_shows_the_toast_and_undo_restores_both() {
    let mut rt = runtime();
    let target = selected_key(&rt.state);
    assert!(keyed(&rt.elements(), "toast").is_none());
    type_keys(&mut rt, "e");
    settle(&mut rt);
    let els = rt.elements();
    assert!(keyed(&els, &target).is_none(), "the archived row is gone");
    assert_eq!(text_in(&els, "toast"), "archived 1 · u undo");
    type_keys(&mut rt, "u");
    settle(&mut rt);
    let els = rt.elements();
    assert!(keyed(&els, &target).is_some(), "the row is back");
    assert_eq!(text_in(&els, "toast"), "undone: archived 1");
    assert_eq!(selected_rows(&rt), [target]);
}

#[test]
fn the_selection_scrolls_into_view() {
    let mut rt = runtime();
    type_keys(&mut rt, "G");
    settle(&mut rt);
    let key = selected_key(&rt.state);
    let els = rt.elements();
    let list = els.iter().find(|e| e.role == "list").unwrap();
    let row = keyed(&els, &key).expect("the last row is mounted once selected");
    let (top, bottom) = (list.rect[1], list.rect[1] + list.rect[3]);
    assert!(row.rect[1] >= top - 0.5 && row.rect[1] + row.rect[3] <= bottom + 0.5, "row {:?} inside list {:?}", row.rect, list.rect);
    assert!(els.iter().filter(|e| e.role == "listitem").count() < 80, "only a window of the rows is mounted");
}

#[test]
fn everything_sits_on_the_cell_grid() {
    let rt = runtime();
    let g = rt.state.frame.get().grid;
    assert_eq!((g.cell_w, g.cell_h), (8.5, 19.0));
    let on_grid = |v: f32, cell: f32| ((v / cell).round() * cell - v).abs() < 0.01;
    let els = rt.elements();
    for e in els.iter().filter(|e| e.role == "listitem") {
        assert!(on_grid(e.rect[0], g.cell_w) && on_grid(e.rect[2], g.cell_w), "row x/w {:?}", e.rect);
        assert!((e.rect[3] - g.y(2)).abs() < 0.01, "rows are two cells tall: {:?}", e.rect);
    }
    for key in ["pane:1", "pane:2", "status", "list-title", "reader-from"] {
        let Some(e) = keyed(&els, key) else { continue };
        assert!(on_grid(e.rect[0], g.cell_w) && on_grid(e.rect[1], g.cell_h), "{key} origin {:?}", e.rect);
        assert!(on_grid(e.rect[2], g.cell_w) && on_grid(e.rect[3], g.cell_h), "{key} size {:?}", e.rect);
    }
    assert!(keyed(&els, "pane:1").is_some() && keyed(&els, "status").is_some());
}

#[test]
fn enter_opens_the_message_in_the_reader_and_the_tab_follows() {
    let mut rt = runtime();
    assert!(keyed(&rt.elements(), "wallpaper").is_some(), "an empty reader shows the wallpaper");
    let ravens = rt.elements().iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("raven"))).count();
    assert_eq!(ravens, 1, "the raven is one canvas element");
    assert!(rt.elements().len() < 400, "not an element per dot: {} elements", rt.elements().len());
    let subject = rt.state.selected().unwrap().subject.clone();
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    let els = rt.elements();
    assert!(keyed(&els, "wallpaper").is_none());
    assert!(text_in(&els, "reader-subject").contains(&subject));
    assert!(text_in(&els, "reader-from").contains("view: html"));
    assert_eq!(keyed(&els, "tab:1").and_then(|e| e.label.clone()), Some(subject));
    assert_eq!(els.iter().find(|e| e.role == "document").and_then(|e| e.key.clone()).as_deref(), Some("reader:2"));
    // The status line moved to the focused pane.
    let (status, reader) = (keyed(&els, "status").unwrap(), keyed(&els, "pane:2").unwrap());
    assert!(status.rect[0] >= reader.rect[0] - 0.5);
}

#[test]
fn j_and_k_scroll_the_reader_by_whole_rows_within_its_extent() {
    // A short window, so the message is taller than the reader.
    let mut rt = runtime_with(AppState::demo_inline(), (1400.0, 260.0));
    let g = rt.state.frame.get().grid;
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    let extent = rt.state.ws().reader_extent.get();
    assert!(extent > 0, "the body overflows the pane");
    let top = |rt: &Runtime<AppState>| keyed(&rt.elements(), "reader-from").map(|e| e.rect[1]);
    let start = top(&rt).expect("the header is on screen");
    type_keys(&mut rt, "j");
    settle(&mut rt);
    assert_eq!(rt.state.ws().reader_top, 3.min(extent));
    let moved = start - top(&rt).expect("still mounted");
    assert!((moved - g.y(3.min(extent))).abs() < 0.5, "scrolled {moved} pt");
    type_keys(&mut rt, "G");
    settle(&mut rt);
    assert_eq!(rt.state.ws().reader_top, extent, "G stops at the end");
    type_keys(&mut rt, "gg");
    settle(&mut rt);
    assert_eq!(rt.state.ws().reader_top, 0);
    assert!((top(&rt).unwrap() - start).abs() < 0.5, "back at the top");
}

#[test]
fn a_narrow_window_shows_one_pane() {
    let mut rt = scene("narrow", (800.0, 700.0));
    let panes = |rt: &Runtime<AppState>| rt.elements().iter().filter(|e| !e.exiting).filter_map(|e| e.key.clone()).filter(|k| k.starts_with("pane:")).collect::<Vec<_>>();
    assert_eq!(panes(&rt), ["pane:1"]);
    assert!(rt.state.frame.get().narrow);
    press(&mut rt, Key::Enter);
    assert_eq!(panes(&rt), ["pane:2"], "enter shows the reader");
    press(&mut rt, Key::Escape);
    assert_eq!(panes(&rt), ["pane:1"], "escape goes back to the list");
}

#[test]
fn dragging_the_gutter_resizes_panes_in_whole_cells() {
    let mut rt = runtime();
    let g = rt.state.frame.get().grid;
    let width = |rt: &Runtime<AppState>| keyed(&rt.elements(), "pane:1").unwrap().rect[2];
    let before = width(&rt);
    let (x, y) = rt.find("gutter:0").expect("a gutter between the panes");
    rt.pointer_down(x, y);
    rt.pointer_move(x - 100.0, y);
    rt.frame(FRAME);
    rt.pointer_up(x - 100.0, y);
    settle(&mut rt);
    let after = width(&rt);
    let moved = (before - after) / g.cell_w;
    assert!((moved - moved.round()).abs() < 0.01 && (11.0..=12.0).contains(&moved.round()), "moved {moved} cells");
    assert!(((after / g.cell_w).round() * g.cell_w - after).abs() < 0.01, "the list is still a whole number of cells wide");
}

#[test]
fn clicking_a_row_selects_it_and_a_tab_or_workspace_switches() {
    let mut rt = runtime();
    let third = AppState::row_key(1, &rt.state.rows()[2].id);
    let (x, y) = rt.find(&third).expect("the third row");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert_eq!(rt.state.ws().sel, 2);
    assert_eq!(selected_rows(&rt), [third]);
    let (x, y) = rt.find("ws:2").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert_eq!(rt.state.ws().name, "Gmail");
    let (x, y) = rt.find("rail:rg-theme").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(&mut rt);
    assert_eq!(rt.state.theme.id, "ravenglass-frost");
    assert_eq!(text_in(&rt.elements(), "toast"), "theme: Ravenglass Frost");
}

#[test]
fn the_palette_lists_actions_with_their_keys() {
    let rt = scene("palette", (1400.0, 880.0));
    let els = rt.elements();
    assert_eq!(keyed(&els, "palette").map(|e| e.role), Some("dialog"));
    assert!(text_in(&els, "palette-query").contains("pane"));
    assert!(text_in(&els, "palette:vsplit").contains("Ctrl+W v") || cfg!(target_os = "macos"));
    assert!(els.iter().filter(|e| e.role == "option").count() >= 5);
}

#[test]
fn rail_buttons_tabs_and_workspaces_are_labelled() {
    let rt = runtime();
    let els = rt.elements();
    let buttons: Vec<_> = els.iter().filter(|e| e.role == "button").collect();
    assert_eq!(buttons.len(), 9);
    assert!(buttons.iter().all(|e| e.label.as_ref().is_some_and(|l| !l.is_empty())), "every icon button has a label");
    for key in ["tab:0", "ws:1", "ws:2", "ws:6", "sync"] {
        assert!(keyed(&els, key).and_then(|e| e.label.as_ref()).is_some(), "{key} has a label");
    }
}

#[test]
fn both_themes_draw_every_role_they_use() {
    for (name, theme) in [("night", ravenglass_theme::Theme::night()), ("frost", ravenglass_theme::Theme::frost())] {
        let magenta = rune::color::hex("#ff00ff").unwrap();
        let paint = Paint::of(&theme, &crate::modes::Mode::default());
        let all = format!("{paint:?}");
        assert!(!all.contains(&format!("{magenta:?}")), "{name}: a role the views ask for is missing from the theme");
        for mode in ["mode.normal", "mode.visual", "mode.search", "mode.command", "mode.compose", "ui.onAccent"] {
            assert!(theme.roles.contains_key(mode), "{name} defines {mode}");
        }
    }
    let rt = scene("frost", (1400.0, 880.0));
    let root = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(view::ROOT)).unwrap();
    assert_eq!(root.fill, Paint::of(&rt.state.theme, &rt.state.mode).bg);
    assert!(root.fill.r > 0.8, "Frost is light");
}

#[test]
fn every_scene_draws() {
    for (name, _) in crate::scenes::SCENES {
        let rt = scene(name, if name.starts_with("narrow") { (800.0, 700.0) } else { (1400.0, 880.0) });
        let els = rt.elements();
        // The lock screen is the wallpaper alone, by design (`tests/lock.rs`).
        if *name == "locked" { continue; }
        assert!(keyed(&els, "tabs").is_some() && keyed(&els, "workspaces").is_some(), "{name} has its chrome");
        assert!(els.len() > 20, "{name} draws something");
    }
}

#[test]
fn html_mail_shows_as_painted_tiles_and_t_falls_back_to_text() {
    let mut rt = scene("html-newsletter", (1400.0, 880.0));
    let info = rt.state.html.info.expect("the engine laid the message out");
    assert_eq!(rt.state.html.tiles.len() as u32, info.tiles, "every tile arrived");
    assert!(!rt.state.html.failed);
    let els = rt.elements();
    let card = keyed(&els, "reader-html").expect("the HTML card is on screen");
    assert!(card.rect[3] >= info.height as f32 / rt.state.html.scale, "the card is as tall as the document, in whole rows");
    assert!(keyed(&els, "reader-images-blocked").is_some(), "the newsletter's hero image is held back");
    assert!(keyed(&els, "reader-html-note").is_none());

    // A plain-text message has no HTML card, whatever the view setting.
    let plain = scene("default", (1400.0, 880.0));
    assert!(keyed(&plain.elements(), "reader-html").is_none());

    // `t` switches the same message to the text view.
    press(&mut rt, Key::Text("t".into()));
    settle(&mut rt);
    let els = rt.elements();
    assert!(keyed(&els, "reader-html").is_none());
    assert!(text_in(&els, &AppState::reader_key(rt.state.ws().panes.focus())).contains("Blitz 0.3 is out") || els.iter().any(|e| e.text.as_deref().is_some_and(|t| t.contains("Blitz 0.3 is out"))));
}

#[test]
fn allowing_images_lays_the_message_out_again() {
    let mut rt = scene("html-newsletter", (1400.0, 880.0));
    let before = rt.state.html.info;
    rt.state.html.info = None;
    press(&mut rt, Key::Text("i".into()));
    settle(&mut rt);
    // The demo has no network, so the image stays a placeholder, but the view is rendered anew.
    assert_eq!(rt.state.html.info, before);
    assert!(rt.state.toast.is_some());
}

//! Headless runs of the real view through Rune's `Runtime`: what is on screen, as `--tree`
//! reports it, before and after keys.

use crate::state::AppState;
use crate::view::{self, Paint};
use rune::runtime::ElementInfo;
use rune::text::TextSystem;
use rune::{Env, Key, KeyEvent, Mods, Runtime};

const FRAME: f64 = 1.0 / 60.0;

fn runtime_with(state: AppState, size: (f32, f32)) -> Runtime<AppState> {
    let text = TextSystem::new(crate::FONTS, crate::FAMILIES);
    let mut rt = Runtime::with_text(state, view::root, size, Env { system_dark: true, scale: 2.0 }, text);
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt.frame(FRAME);
    rt
}

fn runtime() -> Runtime<AppState> { runtime_with(AppState::demo_inline(), (1400.0, 880.0)) }

fn scene(name: &str, size: (f32, f32)) -> Runtime<AppState> {
    let mut state = AppState::demo_inline();
    let (_, setup) = crate::scenes::SCENES.iter().find(|(n, _)| *n == name).expect("a known scene");
    setup(&mut state);
    runtime_with(state, size)
}

fn settle(rt: &mut Runtime<AppState>) {
    for _ in 0..90 { rt.frame(FRAME); }
}

fn press(rt: &mut Runtime<AppState>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(FRAME);
}

fn type_keys(rt: &mut Runtime<AppState>, text: &str) {
    for c in text.chars() { press(rt, Key::Text(c.to_string())); }
}

fn keyed<'a>(els: &'a [ElementInfo], key: &str) -> Option<&'a ElementInfo> { els.iter().find(|e| e.key.as_deref() == Some(key)) }

/// The text drawn inside the element keyed `key`.
fn text_in(els: &[ElementInfo], key: &str) -> String {
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
    assert!(keyed(&rt.elements(), "raven").is_some(), "drawn as one canvas");
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
        assert!(keyed(&els, "tabs").is_some() && keyed(&els, "workspaces").is_some(), "{name} has its chrome");
        assert!(els.len() > 20, "{name} draws something");
    }
}

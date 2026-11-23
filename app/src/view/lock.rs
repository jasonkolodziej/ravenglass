//! The lock screen: the wallpaper and one line, and nothing else. No list, no reader, no tab
//! titles, no toast: while the app is locked the view builds nothing that holds message content.

use super::{ROOT, Ui, wallpaper};
use crate::grid::Rect;
use crate::state::AppState;
use rune::{El, el};

/// What the lock line says. The password shows as one `•` per character.
pub fn caption(s: &AppState) -> String {
    let lock = &s.shell.lock;
    if !lock.prompting() { return "Locked · press any key".into(); }
    let who = s.shell.verifier().map(|id| id.0).unwrap_or_default();
    format!("Locked · {who} password  {}_", lock.input.masked())
}

fn hints(s: &AppState) -> &'static str {
    let lock = &s.shell.lock;
    match (lock.prompting(), lock.checking, lock.refused) {
        (false, ..) => "",
        (true, true, _) => "checking",
        (true, false, true) => "that was not the password    enter unlock    esc back",
        (true, false, false) => "enter unlock    esc back",
    }
}

pub fn screen(s: &AppState, ui: &Ui, cols: usize, rows: usize) -> El {
    let caption = caption(s);
    el().key(ROOT).w_full().h_full().fill(ui.p.bg)
        .child(el().key("lock").role("region").label("Locked").absolute(0.0, 0.0).size(ui.g.x(cols), ui.g.y(rows)).window_drag()
            .child(wallpaper::pane(ui, "lock", Rect::new(0, 0, cols, rows), &caption, hints(s))))
}

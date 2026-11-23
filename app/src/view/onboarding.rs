//! The pane a profile with no account opens on: the wallpaper, and how to add the first account.
//! The status line stays under it, so the `:` command line shows what is typed.

use super::{BAR_ROWS, Ui, statusline, wallpaper};
use crate::grid::Rect;
use crate::state::AppState;
use rune::{El, el};

pub const CAPTION: &str = "No accounts yet";
pub const HINTS: &str = ":account add you@example.com    start with --demo for sample mail";

/// The title bar with no tabs: there is nothing open to name.
pub fn title_bar(ui: &Ui, cols: usize) -> El {
    el().key("tabs").role("tablist").absolute(0.0, 0.0).size(ui.g.x(cols), ui.g.y(BAR_ROWS)).window_drag()
}

pub fn pane(s: &AppState, ui: &Ui, area: Rect) -> El {
    let g = &ui.g;
    let content = Rect::new(0, 0, area.w, area.h.saturating_sub(1));
    let mut e = ui.place(el().key("onboarding").role("region").label("Welcome"), area)
        .child(wallpaper::pane(ui, "onboarding", content, CAPTION, HINTS));
    if area.h > 3 { e = e.child(statusline::line(s, ui, area.w).absolute(0.0, g.y(area.h - 1))); }
    e
}

//! One-line toasts (`archived 1 · u undo`). They sit in the workspace bar's row, just left of the
//! sync state, so they never cover a pane's status line.

use super::Ui;
use super::line::Line;
use crate::grid;
use crate::state::Toast;
use rune::{El, LiveRegion};

/// Cells the toast takes when cut to at most `max` cells.
pub fn width(toast: &Toast, max: usize) -> usize { grid::width(&grid::clip(&toast.text, max.saturating_sub(2))) + 2 }

/// The toast as a one-row block at most `max` cells wide.
pub fn toast(ui: &Ui, toast: &Toast, max: usize) -> El {
    let p = &ui.p;
    let text = grid::clip(&toast.text, max.saturating_sub(2));
    let width = grid::width(&text) + 2;
    let ink = if toast.error { p.error } else { p.text };
    Line::new().gap(1).put(&text, ink).gap(1).fill(0, width, p.chrome).el(ui, width)
        .key("toast").role("status").label(text).live_region(LiveRegion::Polite)
}

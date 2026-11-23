//! One-line toasts above the workspace bar: `archived 1 · u undo`.

use super::Ui;
use super::line::Line;
use crate::grid;
use crate::state::Toast;
use rune::{El, LiveRegion, el};

/// A line `cols` cells wide with the toast's block at its right end.
pub fn toast(ui: &Ui, toast: &Toast, cols: usize) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let text = grid::clip(&toast.text, cols.saturating_sub(4));
    let width = grid::width(&text) + 2;
    let ink = if toast.error { p.error } else { p.text };
    let line = Line::new().gap(1).put(&text, ink).gap(1).fill(0, width, p.chrome).el(ui, width);
    el().size(g.x(cols), g.cell_h).inert().child(
        line.key("toast").role("status").label(text).live_region(LiveRegion::Polite).absolute(g.x(cols.saturating_sub(width + 1)), 0.0),
    )
}

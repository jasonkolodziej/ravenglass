//! The command palette: a centred outlined box over the grid listing every action that matches
//! the query, with its keys. It reads the same catalog the `:` command line parses.

use super::Ui;
use super::line::{Line, Mark};
use crate::actions;
use crate::grid;
use crate::keys;
use rune::{El, el};

/// Rows of matches shown at once.
pub const VISIBLE: usize = 12;
const MAX_COLS: usize = 64;

pub fn palette(ui: &Ui, query: &str, selected: usize, cols: usize, rows: usize) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let matches = actions::palette_matches(query);
    let width = MAX_COLS.min(cols.saturating_sub(4)).max(20);
    let visible = VISIBLE.min(rows.saturating_sub(8)).max(1);
    let height = visible + 3;
    let (x, y) = (cols.saturating_sub(width) / 2, (rows.saturating_sub(height) / 3).max(1));
    // Keep the selection in the window of rows shown.
    let first = selected.saturating_sub(visible - 1).min(matches.len().saturating_sub(visible));

    let mut list = el()
        .key("palette")
        .role("dialog")
        .label("Command palette")
        .absolute(g.x(x), g.y(y))
        .size(g.x(width), g.y(height))
        .fill(p.bg)
        .border(1.0, p.accent);
    let prompt = Line::new().gap(1).put("> ", p.accent).put(&grid::clip(query, width.saturating_sub(6)), p.text).mark(Mark::Caret, p.accent);
    list = list.child(prompt.el(ui, width).key("palette-query").absolute(0.0, (g.cell_h / 2.0).floor()));
    list = list.child(el().absolute(1.0, g.y(2)).size(g.x(width) - 2.0, 1.0).fill(p.border));
    if matches.is_empty() {
        list = list.child(Line::new().gap(2).put("no matching action", p.muted).el(ui, width).absolute(0.0, g.y(2) + (g.cell_h / 2.0).floor()));
    }
    for (i, entry) in matches.iter().enumerate().skip(first).take(visible) {
        let hint = keys::hint(&entry.action);
        let hint_cols = grid::width(&hint);
        let title_cols = width.saturating_sub(hint_cols + 6);
        let active = i == selected;
        let mut line = Line::new().gap(2).put(&grid::fit(entry.title, title_cols), if active { p.text } else { p.muted });
        line = line.to(width.saturating_sub(hint_cols + 2)).put(&hint, if active { p.accent } else { p.muted });
        if active { line = line.fill(0, width, p.selection); }
        let row = line.el(ui, width).key(format!("palette:{}", entry.name)).role("option").label(entry.title).checked(active);
        // Inset one pixel so the selection fill stays inside the outline.
        list = list.child(row.absolute(0.0, g.y(2) + (g.cell_h / 2.0).floor() + g.y(i - first)));
    }
    list
}

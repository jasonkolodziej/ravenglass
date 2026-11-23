//! The title bar: dot-prefixed tabs for the workspace's open things (its list, and the message
//! in the reader). The active tab is bright, the rest muted.

use super::line::Line;
use super::{BAR_ROWS, Ui};
use crate::grid;
use crate::panes::Kind;
use crate::state::{AppState, Source};
use rune::{El, el};

/// Cells kept clear for the window buttons, where the platform draws them inside the content.
const INSET_COLS: usize = if cfg!(target_os = "macos") { 10 } else { 2 };
const TAB_MAX: usize = 36;

/// The tabs of the active workspace: (title, the pane kind it focuses).
pub fn titles(s: &AppState) -> Vec<(String, Kind)> {
    let ws = s.ws();
    let list = match (&ws.search, &ws.source) {
        (Some(search), _) => format!("/{}", search.query),
        (None, Source::Mailbox { account, name }) => format!("{name} — {account}"),
        (None, Source::Search(q)) => q.clone(),
    };
    let mut out = vec![(list, Kind::List)];
    if let Some(open) = &ws.open { out.push((open.subject.clone(), Kind::Reader)); }
    out
}

pub fn bar(s: &AppState, ui: &Ui, cols: usize) -> El {
    let g = &ui.g;
    let active = if s.ws().open.is_some() && s.focused_kind() == Kind::Reader { 1 } else { 0 };
    let mut bar = el().key("tabs").role("tablist").absolute(0.0, 0.0).size(g.x(cols), g.y(BAR_ROWS));
    let mut col = INSET_COLS;
    for (i, (title, kind)) in titles(s).into_iter().enumerate() {
        let title = grid::clip(&title, TAB_MAX);
        let ink = if i == active { ui.p.text } else { ui.p.muted };
        let width = grid::width(&title) + 3;
        if col + width > cols { break; }
        let tab = Line::new().put("•", ink).gap(2).put(&title, ink).el(ui, width)
            .key(format!("tab:{i}")).role("tab").label(title).checked(i == active)
            .absolute(g.x(col), g.cell_h / 2.0)
            .on_press(move |s: &mut AppState| { s.ws_mut().panes.focus_kind(kind); });
        bar = bar.child(tab);
        col += width + 3;
    }
    bar
}

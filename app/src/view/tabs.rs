//! The title bar: dot-prefixed tabs for the workspace's open things (its list, and the message
//! in the reader). The active tab is bright, the rest muted.

use super::line::Line;
use super::{BAR_ROWS, Ui};
use crate::grid;
use crate::panes::Kind;
use crate::state::{AppState, Source};
use rune::{El, el};

/// Cells before the first tab when the platform draws no window buttons inside the content.
const INSET_COLS: usize = 2;
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

/// `inset` is the width in points the platform's window buttons take at the left of the title
/// bar (macOS traffic lights under a full-size content view), or 0.
pub fn bar(s: &AppState, ui: &Ui, cols: usize, inset: f32) -> El {
    let g = &ui.g;
    let drafting = s.focused_kind() == Kind::Compose;
    let active = if drafting { usize::MAX } else if s.ws().open.is_some() && s.focused_kind() == Kind::Reader { 1 } else { 0 };
    let mut bar = el().key("tabs").role("tablist").absolute(0.0, 0.0).size(g.x(cols), g.y(BAR_ROWS)).window_drag();
    let mut col = if inset > 0.0 { (inset / g.cell_w).ceil() as usize + 3 } else { INSET_COLS };
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
    // Open drafts follow the workspace's own tabs.
    for (i, draft) in s.compose.drafts.iter().enumerate() {
        let title = grid::clip(&draft.tab_title(), TAB_MAX);
        let shown = drafting && i == s.compose.active;
        let ink = if shown { ui.p.text } else { ui.p.muted };
        let width = grid::width(&title) + 3;
        if col + width > cols { break; }
        let tab = Line::new().put("•", ink).gap(2).put(&title, ink).el(ui, width)
            .key(format!("tab:draft:{i}")).role("tab").label(title).checked(shown)
            .absolute(g.x(col), g.cell_h / 2.0)
            .on_press(move |s: &mut AppState| s.show_draft(i, false));
        bar = bar.child(tab);
        col += width + 3;
    }
    bar
}

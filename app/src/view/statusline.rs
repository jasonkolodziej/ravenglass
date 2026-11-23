//! The vim-style status line of the focused pane: a coloured mode block, context segments, and a
//! filetype-style block on the right. SEARCH and the `:` command line type into it.

use super::Ui;
use super::line::{Line, Mark};
use crate::grid;
use crate::keys;
use crate::modes::Mode;
use crate::state::{AppState, Source};
use mail_core::Flags;
use mail_store::Query;
use rune::El;

/// The right-hand block.
const KIND: &str = "mail";

pub fn line(s: &AppState, ui: &Ui, cols: usize) -> El {
    let p = &ui.p;
    let mut left = Line::new().block(s.mode.label(), p.on_accent, p.mode);
    let kind_cols = grid::width(KIND) + 2;
    let mut right = String::new();
    match &s.mode {
        Mode::Search { query } => {
            left = left.gap(1).put("/", p.muted);
            left = query_line(left, query, ui).mark(Mark::Caret, p.accent);
            if let Some(search) = &s.ws().search {
                right = if search.pending { "searching".into() } else { format!("{} found", search.results.len()) };
            }
        }
        Mode::Command { line } => {
            left = left.gap(1).put(":", p.muted).put(line, p.text).mark(Mark::Caret, p.accent);
        }
        Mode::Palette { .. } => left = left.gap(1).put("palette", p.muted),
        Mode::Compose => left = left.gap(1).put("esc normal", p.muted),
        Mode::Normal { .. } | Mode::Visual { .. } => {
            let ws = s.ws();
            let path = match (&ws.search, &ws.source) {
                (Some(search), _) => format!("/{}", search.query),
                (None, Source::Mailbox { account, name }) => format!("{account}/{name}"),
                (None, Source::Search(q)) => q.clone(),
            };
            let rows = s.rows();
            let unread = match (&ws.search, s.folder()) {
                (None, Some(folder)) => folder.unread as usize,
                _ => rows.iter().filter(|e| e.is_unread()).count(),
            };
            let flagged = rows.iter().filter(|e| e.flags.contains(Flags::FLAGGED)).count();
            left = left.gap(1).put(&path, p.text).gap(2).put("by date", p.muted).gap(2);
            left = left.mark(Mark::Dot, p.unread).gap(1).put(&unread.to_string(), p.unread).gap(2);
            left = left.mark(Mark::Flag, p.flag).gap(1).put(&flagged.to_string(), p.flag);
            if matches!(s.mode, Mode::Visual { .. }) {
                let n = ws.selected_range().count();
                left = left.gap(2).put(&format!("{n} selected"), p.mode);
            }
            let total = match (&ws.search, s.folder()) {
                (None, Some(folder)) => (folder.total as usize).max(rows.len()),
                _ => rows.len(),
            };
            let at = if rows.is_empty() { 0 } else { ws.sel + 1 };
            let pending: Vec<String> = s.mode.pending().iter().map(|k| {
                keys::KeySpec { key: k.key, ctrl: k.ctrl && !k.cmd, cmd: k.cmd, alt: k.alt, shift: false }.label()
            }).collect();
            right = format!("{}{at}/{total}", if pending.is_empty() { String::new() } else { format!("{}  ", pending.join(" ")) });
        }
    }
    // Chrome fill under everything after the mode block; the kind block closes the line.
    let block_end = grid::width(s.mode.label()) + 2;
    let right_at = cols.saturating_sub(kind_cols + grid::width(&right) + 2);
    let mut out = left;
    if out.cols() + 1 < right_at {
        out = out.to(right_at).put(&right, p.muted).gap(2);
    }
    out = out.to(cols.saturating_sub(kind_cols)).block(KIND, p.on_accent, p.brand).fill(block_end, cols.saturating_sub(kind_cols), p.chrome);
    out.el(ui, cols).key("status").role("status").label(format!("{} mode", s.mode.label()))
}

/// The search query with its operator tokens (`from:`, `is:unread`) in the accent colour.
fn query_line(mut line: Line, query: &str, ui: &Ui) -> Line {
    let (_, tokens) = Query::parse_tokens(query);
    let mut at = 0;
    for token in tokens {
        let (start, end) = (token.start.min(query.len()), token.end.min(query.len()));
        if start < at || !query.is_char_boundary(start) || !query.is_char_boundary(end) { continue; }
        line = line.put(&query[at..start], ui.p.text);
        line = line.put(&query[start..end], if token.operator { ui.p.accent } else { ui.p.text });
        at = end;
    }
    line.put(&query[at..], ui.p.text)
}

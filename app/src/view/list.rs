//! The message list pane: a title line, then two-line rows in a virtualized scroll view
//! (relative-number gutter, unread dot, sender, subject, flag and attachment marks, time; the
//! preview on the second line).

use super::line::{Line, Mark};
use super::{Ui, wallpaper};
use crate::actions::Action;
use crate::grid::{self, Rect};
use crate::state::{AppState, Source};
use crate::timefmt;
use mail_core::{Flags, MessageId};
use rune::{El, GrowthSpec, ScrollSpec, col, el};
use std::rc::Rc;

/// Cells of each column of a row, left to right.
const NUM_COLS: usize = 3;
const SENDER_COL: usize = 7;
const SENDER_COLS: usize = 16;
const SUBJECT_COL: usize = SENDER_COL + SENDER_COLS + 1;
/// space, flag, space, attachment, space, time, space.
const TIME_COLS: usize = 6;
const TAIL_COLS: usize = 6 + TIME_COLS;
/// Rows of the pane above the list: the title and a blank line.
const HEAD_ROWS: usize = 2;

pub fn pane(s: &AppState, ui: &Rc<Ui>, pane: u32, area: Rect) -> El {
    let g = &ui.g;
    let ws = s.ws();
    let rows = s.rows();
    let mut e = el().size(g.x(area.w), g.y(area.h)).child(title(s, ui, area.w));
    // Whole two-line rows only: an odd row at the bottom stays empty.
    let body = Rect::new(0, HEAD_ROWS, area.w, area.h.saturating_sub(HEAD_ROWS) / 2 * 2);
    if rows.is_empty() {
        let caption = match &ws.search {
            Some(search) if search.pending => "Searching…",
            Some(_) => "No matches",
            None if ws.folder.is_none() => "Connecting…",
            None => "No messages",
        };
        let body = Rect::new(0, HEAD_ROWS, area.w, area.h.saturating_sub(HEAD_ROWS));
        return e.child(wallpaper::pane(ui, body, caption, &wallpaper::hints()).absolute(0.0, g.y(HEAD_ROWS)));
    }
    let (ui2, cols) = (ui.clone(), area.w);
    let list = col()
        .key(format!("rows:{pane}"))
        .w(g.x(area.w))
        .growth(GrowthSpec::default())
        .virtual_rows(
            rows.len(),
            g.y(2),
            move |s: &AppState, i| match s.rows().get(i) {
                Some(e) => AppState::row_key(pane, &e.id).into(),
                None => format!("m:{pane}:#{i}").into(),
            },
            move |s: &AppState, i| row(s, &ui2, i, cols),
        );
    let view = col()
        .key(AppState::list_key(pane))
        .role("list")
        .label(match &ws.source { Source::Mailbox { name, .. } => name.clone(), Source::Search(q) => q.clone() })
        .absolute(0.0, g.y(HEAD_ROWS))
        .size(g.x(body.w), g.y(body.h))
        .scroll(ScrollSpec { stick_bottom: false })
        .child(list);
    e = e.child(view);
    e
}

/// `INBOX  iCloud · 248 · 12 unread · by date`.
fn title(s: &AppState, ui: &Ui, cols: usize) -> El {
    let p = &ui.p;
    let ws = s.ws();
    let count = s.rows().len();
    let line = match (&ws.search, &ws.source, s.folder()) {
        (Some(search), _, _) => {
            let found = if search.pending && search.results.is_empty() { "searching".to_owned() } else { format!("{count} found") };
            Line::new().gap(1).block("SEARCH", p.on_accent, p.accent).gap(1).put(&format!("{} · {found}", search.query), p.muted)
        }
        (None, Source::Mailbox { account, name }, folder) => {
            let detail = match folder {
                Some(f) => format!("{account} · {} · {} unread · by date", f.total, f.unread),
                None => format!("{account} · connecting"),
            };
            Line::new().gap(1).block(&name.to_uppercase(), p.on_accent, p.accent).gap(1).put(&detail, p.muted)
        }
        (None, Source::Search(q), _) => Line::new().gap(1).block("SEARCH", p.on_accent, p.accent).gap(1).put(q, p.muted),
    };
    line.el(ui, cols).key("list-title")
}

fn row(s: &AppState, ui: &Ui, i: usize, cols: usize) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let Some(e) = s.rows().get(i) else { return el().size(g.x(cols), g.y(2)) };
    let ws = s.ws();
    let cursor = i == ws.sel;
    let selected = ws.selected_range().contains(&i);
    let unread = e.is_unread();
    let number = if cursor { i + 1 } else { i.abs_diff(ws.sel) };
    let subject_cols = cols.saturating_sub(SUBJECT_COL + TAIL_COLS);

    let mut first = Line::new().gap(1);
    first = if cursor { first.bold(&grid::fit_right(&number.to_string(), NUM_COLS), p.text) } else { first.put(&grid::fit_right(&number.to_string(), NUM_COLS), p.muted) };
    first = first.gap(1);
    first = if unread { first.mark(Mark::Dot, p.unread) } else { first.gap(1) };
    first = first.to(SENDER_COL);
    let sender = grid::fit(e.from.display(), SENDER_COLS);
    first = if unread { first.bold(&sender, p.text) } else { first.put(&sender, p.text) };
    first = first.gap(1).put(&grid::fit(&e.subject, subject_cols), p.text).gap(1);
    first = if e.flags.contains(Flags::FLAGGED) { first.mark(Mark::Flag, p.flag) } else { first.gap(1) };
    first = first.gap(1);
    first = if e.attachments.is_empty() { first.gap(1) } else { first.mark(Mark::Clip, p.attachment) };
    first = first.gap(1).put(&grid::fit_right(&timefmt::short(e.date, s.wall.now()), TIME_COLS), p.muted);

    let second = Line::new().to(SUBJECT_COL).put(&grid::clip(&e.preview, cols.saturating_sub(SUBJECT_COL + 1)), p.muted);

    let id: MessageId = e.id.clone();
    let open_id = id.clone();
    let mut row = col().size(g.x(cols), g.y(2)).role("listitem").label(format!("{}: {}", e.from.display(), e.subject)).checked(selected);
    if selected { row = row.fill(p.selection); }
    row = row.child(first.el(ui, cols)).child(second.el(ui, cols));
    if cursor { row = row.child(el().absolute(0.0, 0.0).size(3.0, g.y(2)).fill(p.accent).inert()); }
    row.on_press(move |s: &mut AppState| select(s, &id))
        .on_double_press_tx(move |s: &mut AppState, tx| {
            select(s, &open_id);
            s.perform(Action::Open);
            crate::flush(s, tx);
        })
}

fn select(s: &mut AppState, id: &MessageId) {
    if let Some(at) = s.rows().iter().position(|e| &e.id == id) {
        let ws = s.ws_mut();
        ws.sel = at;
        ws.panes.focus_kind(crate::panes::Kind::List);
    }
}

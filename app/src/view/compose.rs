//! The compose pane (plan §4 "Compose"): a filled pane title bar, the To / Cc / Subject fields as
//! grid rows, and the body in an outlined prompt box with a hint line. The fields are Rune text
//! fields in the grid's own text style, so every character still takes one cell while caret
//! movement, selection, undo, paste and the input method are Rune's.

use super::line::{Line, Mark, mark_el};
use super::{Ui, wallpaper};
use crate::actions::Action;
use crate::compose::{Completion, Composer, Field};
use crate::grid::{self, Rect};
use crate::keys::{self, KeyPress};
use crate::modes::Mode;
use crate::state::AppState;
use mail_core::Address;
use rune::text::Span;
use rune::{Color, El, KeyEvent, Tx, auto_grow, edit_access, el, text_input};

/// The pane's element key; the caret of any field inside it is read through it.
pub const KEY: &str = "compose";
/// Width of the header key column (`Subject: `), as in the reader.
const KEY_COLS: usize = 9;
/// Columns left clear on each side.
const PAD: usize = 1;
/// The header fields, one row each under the title bar.
const HEADERS: [Field; 3] = [Field::To, Field::Cc, Field::Subject];
/// Rows the box needs: its two edges, one body row and the hint.
const MIN_BOX_ROWS: usize = 4;
const COMPLETION_COLS: usize = 56;

pub fn pane(s: &AppState, ui: &Ui, pane: u32, area: Rect) -> El {
    let Some(c) = s.compose.current() else {
        return wallpaper::pane(ui, &format!("compose:{pane}"), area, "No draft open", &wallpaper::hints());
    };
    let g = &ui.g;
    let p = &ui.p;
    let cols = area.w;
    let composing = s.mode == Mode::Compose && c.editing();
    let mut root = el().key(KEY).role("form").label(c.title()).size(g.x(cols), g.y(area.h));
    root = root.child(title_bar(s, ui, c, cols));

    // A short pane drops Cc, then keeps only the box.
    let spare = area.h.saturating_sub(1 + MIN_BOX_ROWS);
    let headers: &[Field] = if spare >= HEADERS.len() { &HEADERS } else if spare >= 2 { &[Field::To, Field::Subject] } else { &[] };
    let value_cols = cols.saturating_sub(2 * PAD + KEY_COLS);
    for (i, field) in headers.iter().enumerate() {
        let key = Line::new().gap(PAD).put(&grid::fit(&format!("{}:", field.label()), KEY_COLS), p.header_key);
        root = root.child(key.el(ui, PAD + KEY_COLS).absolute(0.0, g.y(1 + i)));
        let value = if c.editing() {
            input(ui, c, *field).single_line().size(g.x(value_cols), g.cell_h)
        } else {
            Line::new().put(&grid::clip(&c.edit(*field).text, value_cols), p.muted).el(ui, value_cols)
        };
        root = root.child(value.absolute(g.x(PAD + KEY_COLS), g.y(1 + i)));
    }

    // The prompt box: its edges run through the middle of their rows, so the text inside and
    // everything around it stay on whole cells.
    let top = 1 + headers.len();
    let box_rows = area.h.saturating_sub(top).max(MIN_BOX_ROWS);
    let body_rows = box_rows - 3;
    let inner = cols.saturating_sub(4 * PAD);
    let half = (g.cell_h / 2.0).floor();
    // Brighter while the box has the keyboard.
    let edge = if composing { p.muted.alpha(0.7) } else { p.border };
    root = root.child(
        el().key("compose-box").absolute(g.x(PAD), g.y(top) + half).size(g.x(cols.saturating_sub(2 * PAD)), g.y(box_rows - 1)).border(1.0, edge).inert(),
    );
    let body = if c.editing() {
        let field = input(ui, c, Field::Body)
            .decorations(quote_spans(&c.body.text, p.quote))
            .on_submit(|s: &mut AppState, tx: &mut Tx| {
                s.compose.edit_mut(Field::Body).insert("\n");
                changed(s, Field::Body, tx);
            });
        el().size(g.x(inner), g.y(body_rows)).child(auto_grow(field, body_rows, body_rows))
    } else {
        let mut lines = el().size(g.x(inner), g.y(body_rows));
        for (i, text) in c.body.text.lines().take(body_rows).enumerate() {
            let ink = if text.starts_with('>') { p.quote } else { p.muted };
            lines = lines.child(Line::new().put(&grid::clip(text, inner), ink).el(ui, inner).absolute(0.0, g.y(i)));
        }
        lines
    };
    root = root.child(body.absolute(g.x(2 * PAD), g.y(top + 1)));
    root = root.child(Line::new().put(&grid::clip(&hint(s, c), inner), p.muted).el(ui, inner).key("compose-hint").absolute(g.x(2 * PAD), g.y(top + 1 + body_rows)));

    if let Some(list) = c.completion.as_ref().filter(|l| headers.contains(&l.field)) {
        let row = 2 + headers.iter().position(|f| *f == list.field).unwrap_or(0);
        root = root.child(completion(ui, list, value_cols).absolute(g.x(PAD + KEY_COLS), g.y(row)));
    }
    root
}

/// `⠿   reply ▸ Odin Allfather   draft ✓` on a filled row.
fn title_bar(s: &AppState, ui: &Ui, c: &Composer, cols: usize) -> El {
    let p = &ui.p;
    let (state, ink) = match (&c.outbox, s.sending_label()) {
        (Some(_), Some(label)) => (label, p.flag),
        _ if c.unsaved > 0 => ("draft …".to_owned(), p.muted),
        _ => ("draft ✓".to_owned(), p.ok),
    };
    let state_at = cols.saturating_sub(grid::width(&state) + 2);
    let title = grid::clip(&c.title(), state_at.saturating_sub(6));
    // Centred in the pane, unless that would run into the state on the right.
    let title_at = (cols.saturating_sub(grid::width(&title)) / 2).min(state_at.saturating_sub(grid::width(&title) + 2)).max(4);
    Line::new().gap(1).mark(Mark::Grip, p.muted).to(title_at).put(&title, p.text).to(state_at).put(&state, ink)
        .fill(0, cols, p.chrome)
        .el(ui, cols)
        .key("compose-title")
}

fn hint(s: &AppState, c: &Composer) -> String {
    let send = keys::hint(&Action::Send);
    if !c.editing() {
        "u undo send".to_owned()
    } else if s.mode == Mode::Compose {
        format!("{send} send · esc normal")
    } else {
        format!("enter edit · {send} send · :discard")
    }
}

/// One field's editor. Keys reach the app first (`esc`, `tab`, the send chord, the suggestion
/// list); the rest is the editor's.
fn input(ui: &Ui, c: &Composer, field: Field) -> El {
    let p = &ui.p;
    text_input(c.edit(field), ui.g.style(400.0))
        .key(field.key())
        .label(field.label())
        .color(p.text)
        .caret(p.accent, p.selection)
        .on_edit(edit_access(move |s: &mut AppState| s.compose.edit_mut(field)))
        .on_input_key(move |s: &mut AppState, ev: &KeyEvent, tx: &mut Tx| {
            let Some(press) = KeyPress::from_event(ev) else { return false };
            s.clock = tx.clock();
            let took = s.compose_input_key(field, press);
            crate::flush(s, tx);
            took
        })
        .on_input_change(move |s: &mut AppState, tx: &mut Tx| changed(s, field, tx))
}

fn changed(s: &mut AppState, field: Field, tx: &mut Tx) {
    s.clock = tx.clock();
    s.compose_changed(field);
    crate::flush(s, tx);
}

/// Quoted lines of the body in the quote colour.
fn quote_spans(text: &str, color: Color) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with('>') { spans.push(Span::new(at, at + line.len()).color(color)); }
        at += line.len();
    }
    spans
}

fn address_line(a: &Address) -> String {
    match &a.name { Some(name) if !name.is_empty() => format!("{name} <{}>", a.email), _ => a.email.clone() }
}

/// The suggestions under an address field: `tab` or `enter` takes the bright one.
fn completion(ui: &Ui, list: &Completion, max_cols: usize) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let widest = list.addresses.iter().map(|a| grid::width(&address_line(a))).max().unwrap_or(0);
    let width = (widest + 2).clamp(24, COMPLETION_COLS).min(max_cols.max(1));
    let mut popup = el().key("compose-completion").role("listbox").label("Address suggestions").size(g.x(width), g.y(list.addresses.len())).fill(p.chrome);
    for (i, address) in list.addresses.iter().enumerate() {
        let active = i == list.selected;
        let text = grid::fit(&address_line(address), width.saturating_sub(2));
        let mut line = Line::new().gap(1).put(&text, if active { p.text } else { p.muted });
        if active { line = line.fill(0, width, p.selection); }
        popup = popup.child(line.el(ui, width).key(format!("completion:{i}")).role("option").label(address.display().to_owned()).checked(active).absolute(0.0, g.y(i)));
    }
    popup
}

/// The block caret of the grid, over the focused field's own caret line (`rect`, in window
/// points): one cell, snapped to the grid.
pub fn caret(ui: &Ui, rect: [f32; 4]) -> El {
    let g = &ui.g;
    let x = (rect[0] / g.cell_w).round() * g.cell_w;
    let y = (rect[1] / g.cell_h).floor() * g.cell_h;
    mark_el(g, Mark::Caret, ui.p.accent.alpha(0.55)).key("compose-caret").absolute(x, y)
}

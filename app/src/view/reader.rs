//! The reader pane: header key/value columns, attachment chips, then the body. The body draws
//! the text view model (`mail_render::TextBody`): quotes muted and folded, links underlined in
//! the accent colour. Long lines are wrapped in code, on cell boundaries.

use super::line::{Line, Mark};
use super::{Ui, wallpaper};
use crate::grid::{self, Rect};
use crate::state::{AppState, HtmlPane};
use crate::timefmt;
use mail_core::{Address, Attachment};
use mail_render::{SanitizedHtml, Span, TextBody};
use ravenglass_engine::ReaderView;

/// Tiles are this many device pixels tall (`mail_blitz::TILE_H`).
const TILE_H: u32 = 1024;
use rune::{El, ScrollSpec, col, el};
use std::rc::Rc;

/// Width of the header key column (`Subject: `).
const KEY_COLS: usize = 9;
/// Columns left clear on each side of the text.
const PAD: usize = 1;

/// What the body region draws.
pub enum Surface<'a> {
    /// The grid-native text view. Always used for plain-text mail.
    Text(&'a TextBody),
    /// The HTML view: tiles painted by the render thread, on a paper card. `text` shows while
    /// the tiles are on their way and if the renderer gives up; see [`html_surface`].
    Html { html: &'a SanitizedHtml, text: &'a TextBody },
}

/// Pick the surface for a body: HTML only when the message has it and the HTML view is on.
pub fn surface<'a>(view: ReaderView, text: &'a TextBody, html: Option<&'a SanitizedHtml>) -> Surface<'a> {
    match (view, html) {
        (ReaderView::Html, Some(html)) => Surface::Html { html, text },
        _ => Surface::Text(text),
    }
}

pub fn pane(s: &AppState, ui: &Rc<Ui>, pane: u32, area: Rect) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let ws = s.ws();
    let Some(env) = &ws.open else { return wallpaper::pane(ui, &AppState::reader_key(pane), area, "No message open", &wallpaper::hints()) };
    let body = s.body();
    let cols = area.w;
    let inner = cols.saturating_sub(2 * PAD);
    let mut lines: Vec<El> = Vec::new();

    let (from, to, subject, date, attachments): (Option<&Address>, &[Address], &str, i64, &[Attachment]) = match body {
        Some(b) => (
            b.from.as_ref().or(Some(&env.from)),
            if b.to.is_empty() { &env.to } else { &b.to },
            if b.subject.is_empty() { &env.subject } else { &b.subject },
            b.date.unwrap_or(env.date),
            if b.attachments.is_empty() { &env.attachments } else { &b.attachments },
        ),
        None => (Some(&env.from), &env.to, &env.subject, env.date, &env.attachments),
    };
    let address = |a: &Address| match &a.name { Some(name) => format!("{name} <{}>", a.email), None => a.email.clone() };
    let value_cols = inner.saturating_sub(KEY_COLS);
    let header = |key: &str, value: &str, bold: bool| {
        let line = Line::new().gap(PAD).put(&grid::fit(key, KEY_COLS), p.header_key);
        let value = grid::clip(value, value_cols);
        if bold { line.bold(&value, p.text) } else { line.put(&value, p.text) }
    };

    // From, with the view toggle on the right when there is room for it.
    let toggle_cols = 20; // "view: html │ t text"
    let from_text = from.map(address).unwrap_or_default();
    let mut first = Line::new().gap(PAD).put(&grid::fit("From:", KEY_COLS), p.header_key);
    if value_cols > toggle_cols + 12 {
        // Bright is the view on screen: HTML only for a message that has it, with the HTML view on.
        let shows_html = body.is_some_and(|b| matches!(surface(s.reader_view, &b.text, b.html.as_ref()), Surface::Html { .. }));
        let (html, text) = if shows_html { (p.text, p.muted) } else { (p.muted, p.text) };
        first = first.put(&grid::clip(&from_text, value_cols - toggle_cols - 2), p.text).to(cols - PAD - toggle_cols + 1);
        first = first.put("view: ", p.muted).put("html", html).put(" │ ", p.muted).put("t ", p.muted).put("text", text);
    } else {
        first = first.put(&grid::clip(&from_text, value_cols), p.text);
    }
    lines.push(first.el(ui, cols).key("reader-from"));
    lines.push(header("To:", &to.iter().map(address).collect::<Vec<_>>().join(", "), false).el(ui, cols));
    lines.push(header("Date:", &timefmt::long(date), false).el(ui, cols));
    lines.push(header("Subject:", subject, true).el(ui, cols).key("reader-subject"));
    if !attachments.is_empty() {
        let mut chips = Line::new().gap(PAD);
        for a in attachments {
            let chip = format!("{} · {}", a.name, timefmt::bytes(a.size));
            if chips.cols() + grid::width(&chip) + 6 > cols { break; }
            chips = chips.mark(Mark::Clip, p.attachment).gap(1).put(&chip, p.attachment).gap(4);
        }
        lines.push(chips.el(ui, cols).key("reader-attachments"));
    }
    // A hairline in the middle of its own row.
    lines.push(
        el().size(g.x(cols), g.cell_h)
            .child(el().absolute(g.x(PAD), (g.cell_h / 2.0).floor()).size(g.x(inner), 1.0).fill(p.border))
            .inert(),
    );
    lines.push(blank(ui, cols));

    // What the engine lays HTML out at: the text width in points, at the window's scale.
    s.reader_px.set((g.x(inner), ui.scale));
    // Rows the HTML card takes beyond the one element it is pushed as.
    let mut extra_rows = 0;
    match body {
        None => lines.push(Line::new().gap(PAD).put("loading…", p.muted).el(ui, cols)),
        Some(b) => match surface(s.reader_view, &b.text, b.html.as_ref()) {
            Surface::Text(text) => text_surface(ui, text, ws.quotes_open, cols, &mut lines),
            Surface::Html { html, text } => {
                if html.remote_images > 0 {
                    lines.push(Line::new().gap(PAD).put(&grid::clip("remote images blocked · i load once · I always for sender", inner), p.muted).el(ui, cols).key("reader-images-blocked"));
                    lines.push(blank(ui, cols));
                }
                let ours = s.html.message.as_ref() == Some(&env.id);
                extra_rows = html_surface(ui, ours.then_some(&s.html), text, ws.quotes_open, cols, &mut lines);
            }
        },
    }
    lines.push(blank(ui, cols));

    ws.reader_extent.set((lines.len() + extra_rows).saturating_sub(area.h));
    let content = col().w(g.x(cols)).children(lines);
    col()
        .key(AppState::reader_key(pane))
        .role("document")
        .label(subject.to_owned())
        .size(g.x(area.w), g.y(area.h))
        .scroll(ScrollSpec { stick_bottom: false })
        .child(content)
}

fn blank(ui: &Ui, cols: usize) -> El { el().size(ui.g.x(cols), ui.g.cell_h) }

/// The HTML surface: one element as tall as the laid-out document, rounded up to whole rows,
/// holding each painted tile at its place. Returns the rows it takes beyond the first.
fn html_surface(ui: &Ui, pane: Option<&HtmlPane>, text: &TextBody, quotes_open: bool, cols: usize, out: &mut Vec<El>) -> usize {
    let g = &ui.g;
    let Some((pane, info)) = pane.filter(|p| !p.failed).and_then(|p| Some((p, p.info?))) else {
        // Not laid out (yet), or the renderer gave up: the text view is always there.
        let note = if pane.is_some_and(|p| p.failed) { "simplified view · this message could not be rendered as html" } else { "rendering html…" };
        out.push(Line::new().gap(PAD).put(note, ui.p.muted).el(ui, cols).key("reader-html-note"));
        out.push(blank(ui, cols));
        text_surface(ui, text, quotes_open, cols, out);
        return 0;
    };
    let scale = pane.scale.max(1.0);
    let (width, height) = (info.width as f32 / scale, info.height as f32 / scale);
    let rows = ((height / g.cell_h).ceil() as usize).max(1);
    let mut card = el().key("reader-html").size(g.x(cols), g.y(rows));
    for (index, pixels) in &pane.tiles {
        let top = (index * TILE_H) as f32 / scale;
        let tall = (TILE_H as f32 / scale).min(height - top);
        if tall > 0.0 { card = card.child(el().absolute(g.x(PAD), top).size(width, tall).pixels(pixels)); }
    }
    out.push(card);
    rows - 1
}

fn text_surface(ui: &Ui, text: &TextBody, quotes_open: bool, cols: usize, out: &mut Vec<El>) {
    let p = &ui.p;
    let inner = cols.saturating_sub(2 * PAD);
    let folded = text.folded_lines();
    let mut hidden_run = false;
    for line in &text.lines {
        if line.folded && !quotes_open {
            if !hidden_run {
                hidden_run = true;
                let note = format!("… {folded} more quoted lines · zo show");
                out.push(Line::new().gap(PAD).put(&grid::clip(&note, inner), p.quote).el(ui, cols).key("reader-fold"));
            }
            continue;
        }
        hidden_run = false;
        let quoted = line.quote_depth > 0;
        let prefix = "> ".repeat(line.quote_depth as usize);
        let width = inner.saturating_sub(grid::width(&prefix)).max(8);
        if line.spans.is_empty() {
            out.push(Line::new().gap(PAD).put(prefix.trim_end(), p.quote).el(ui, cols));
            continue;
        }
        for row in wrap(&line.spans, width) {
            let mut l = Line::new().gap(PAD).put(&prefix, p.quote);
            for (piece, link) in row {
                l = if link { l.link(&piece, p.link) } else { l.put(&piece, if quoted { p.quote } else { p.text }) };
            }
            out.push(l.el(ui, cols));
        }
    }
}

/// Break one logical line into rows of at most `width` cells, at spaces where it can. Each row
/// is a list of (text, is a link).
pub fn wrap(spans: &[Span], width: usize) -> Vec<Vec<(String, bool)>> {
    let width = width.max(1);
    let cells: Vec<(char, bool)> = spans
        .iter()
        .flat_map(|s| match s {
            Span::Text(t) => t.chars().map(|c| (c, false)).collect::<Vec<_>>(),
            Span::Link(t) => t.chars().map(|c| (c, true)).collect::<Vec<_>>(),
        })
        .collect();
    let mut rows: Vec<&[(char, bool)]> = Vec::new();
    let mut start = 0;
    while cells.len() - start > width {
        let window = &cells[start..start + width + 1];
        // The last space that still leaves the row within the width; links break only if they must.
        let cut = window.iter().rposition(|(c, link)| *c == ' ' && !*link);
        let (end, next) = match cut {
            Some(at) if at > 0 => (start + at, start + at + 1),
            _ => (start + width, start + width),
        };
        rows.push(&cells[start..end]);
        start = next;
        while start < cells.len() && cells[start].0 == ' ' { start += 1; }
    }
    if start < cells.len() || rows.is_empty() { rows.push(&cells[start..]); }
    rows.into_iter()
        .map(|row| {
            let mut pieces: Vec<(String, bool)> = Vec::new();
            for (c, link) in row {
                match pieces.last_mut() {
                    Some((text, l)) if l == link => text.push(*c),
                    _ => pieces.push((c.to_string(), *link)),
                }
            }
            pieces
        })
        .collect()
}

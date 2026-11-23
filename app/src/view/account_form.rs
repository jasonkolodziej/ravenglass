//! The add-account form: a centred outlined box over the grid, like the palette. It draws the
//! state of `accounts_ui::Flow` and nothing else; the password shows as one `•` per character.

use super::Ui;
use super::line::{Line, Mark};
use crate::accounts_ui::{self, FieldKind, Flow, Form};
use crate::grid;
use mail_auth::autoconfig::Source;
use rune::{El, el};

const MAX_COLS: usize = 68;
const LABEL_COLS: usize = 16;

fn source_line(form: &Form) -> (&'static str, bool) {
    match form.source {
        Some(Source::Builtin) => ("settings: built in for this provider", false),
        Some(Source::Ispdb) => ("settings: from the provider's autoconfig document", false),
        Some(Source::Guess) => ("settings: guessed from the domain · check them before going on", true),
        None => ("settings: as saved", false),
    }
}

/// The form's rows, top to bottom: (line, row key).
fn rows(ui: &Ui, flow: &Flow, width: usize) -> Vec<(Line, Option<String>)> {
    let p = &ui.p;
    let inner = width.saturating_sub(4);
    let text = |s: &str, ink| (Line::new().gap(2).put(&grid::clip(s, inner), ink), None);
    let mut out: Vec<(Line, Option<String>)> = Vec::new();
    let Some(form) = flow.form() else {
        if let Flow::Discovering { email, .. } = flow {
            out.push((Line::new().gap(2).bold(&grid::clip(&format!("Add account · {email}"), inner), p.text), Some("form-title".into())));
            out.push(text("", p.muted));
            out.push(text("looking up its settings", p.muted));
            out.push(text("", p.muted));
            out.push(text("esc cancel", p.muted));
        }
        return out;
    };
    out.push((Line::new().gap(2).bold(&grid::clip(&format!("Account {} · {}", form.id, form.address), inner), p.text), Some("form-title".into())));
    let (source, guessed) = source_line(form);
    out.push((Line::new().gap(2).put(&grid::clip(source, inner), if guessed { p.warn } else { p.muted }), Some("form-source".into())));
    out.push(text("", p.muted));
    let editing = matches!(flow, Flow::Review { .. });
    for (i, field) in form.fields.iter().enumerate() {
        let focused = editing && i == form.focus;
        let value_cols = inner.saturating_sub(LABEL_COLS + 1);
        let mut line = Line::new().gap(2).put(&grid::fit(field.label, LABEL_COLS), p.header_key);
        let shown = grid::clip(&field.value, value_cols.saturating_sub(4));
        line = match (field.kind, focused) {
            (FieldKind::Choice(_), true) => line.put("‹ ", p.accent).put(&shown, p.text).put(" ›", p.accent),
            (_, true) => line.put(&shown, p.text).mark(Mark::Caret, p.accent),
            (_, false) => line.put(&shown, if editing { p.text } else { p.muted }),
        };
        if focused { line = line.fill(1, width - 1, p.selection); }
        out.push((line, Some(format!("field:{}", field.label))));
    }
    out.push(text("", p.muted));
    if let Some(error) = &form.error { out.push((Line::new().gap(2).put(&grid::clip(error, inner), p.error), Some("form-error".into()))); }
    match flow {
        Flow::Review { .. } => out.push(text("↑↓ field    ←→ change    enter continue    esc cancel", p.muted)),
        Flow::Password { input, .. } => {
            let line = Line::new().gap(2).put(&grid::fit("password", LABEL_COLS), p.header_key).put(&grid::clip(&input.masked(), inner.saturating_sub(LABEL_COLS + 2)), p.text).mark(Mark::Caret, p.accent).fill(1, width - 1, p.selection);
            out.push((line, Some("form-password".into())));
            out.push(text("It is kept in the system keyring, never in a file.", p.muted));
            out.push(text("enter save    esc back", p.muted));
        }
        Flow::NoClientId { provider, .. } => {
            let name = accounts_ui::provider_name(*provider);
            out.push(text(&format!("This build has no {name} OAuth client id, so it cannot"), p.warn));
            out.push(text(&format!("open the {name} sign-in. Set {}.", accounts_ui::client_id_env(*provider)), p.warn));
            if accounts_ui::allows_app_password(*provider) {
                out.push(text("An app password works instead (it needs 2-step verification).", p.muted));
                out.push(text("enter use an app password    esc back", p.muted));
            } else {
                out.push(text(&format!("{name} no longer takes passwords for IMAP and SMTP."), p.muted));
                out.push(text("esc back", p.muted));
            }
        }
        Flow::SigningIn { .. } => {
            out.push(text("Finish signing in in your browser.", p.text));
            out.push(text("esc cancel", p.muted));
        }
        Flow::Saving { .. } => out.push(text("saving", p.muted)),
        Flow::Discovering { .. } => {}
    }
    out
}

pub fn dialog(ui: &Ui, flow: &Flow, cols: usize, rows_total: usize) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let width = MAX_COLS.min(cols.saturating_sub(4)).max(24);
    let lines = rows(ui, flow, width);
    let height = (lines.len() + 2).min(rows_total.saturating_sub(2)).max(3);
    let (x, y) = (cols.saturating_sub(width) / 2, (rows_total.saturating_sub(height) / 3).max(1));
    let mut dialog = el().key("account-form").role("dialog").label("Account").absolute(g.x(x), g.y(y)).size(g.x(width), g.y(height)).fill(p.bg).border(1.0, p.accent);
    for (i, (line, key)) in lines.into_iter().enumerate().take(height.saturating_sub(2)) {
        let mut row = line.el(ui, width).absolute(0.0, g.y(i + 1));
        if let Some(key) = key { row = row.key(key); }
        dialog = dialog.child(row);
    }
    dialog
}

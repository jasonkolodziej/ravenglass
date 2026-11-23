//! The Relay sign-in screen: the wallpaper raven with an outlined box over it, in the grid like
//! the account form. It draws [`Session`] and nothing else; secrets show as one `•` per
//! character, and while the screen is up no mail is built.

use super::session::{Field, Form, Session};
use crate::grid::{self, Grid, Rect};
use crate::state::AppState;
use crate::view::line::{Line, Mark};
use crate::view::{Chrome, Paint, ROOT, Ui, wallpaper};
use rune::{Cx, El, el};

const MAX_COLS: usize = 64;
const LABEL_COLS: usize = 12;

/// The whole window while there is no session. `relay` names the Relay being signed in to.
pub fn root(s: &AppState, session: &Session, relay: &str, cx: &mut Cx<AppState>) -> El {
    let size = cx.layout_size(ROOT).unwrap_or(crate::WINDOW);
    let spec = &s.theme.ui;
    let g = Grid::measure(spec.font_size.unwrap_or(14.0), spec.line_height.unwrap_or(1.35), cx.env().scale);
    let ui = Ui { g, scale: cx.env().scale, p: Paint::of(&s.theme, &s.mode), wallpaper_opacity: spec.wallpaper_opacity.unwrap_or(0.85) };
    s.shell.system_dark.set(Some(cx.env().system_dark));
    let chrome = Chrome::of(&g, size);
    screen(&ui, session, relay, chrome.cols, chrome.rows)
}

pub fn screen(ui: &Ui, session: &Session, relay: &str, cols: usize, rows: usize) -> El {
    let g = &ui.g;
    let (caption, hints) = match session {
        Session::Checking => ("Checking for a relay session", ""),
        Session::Ready(_) => ("Signed in", ""),
        Session::SignIn(_) | Session::SigningIn(_) => ("", ""),
    };
    let mut root = el().key(ROOT).w_full().h_full().fill(ui.p.bg)
        .child(el().key("signin-wallpaper").absolute(0.0, 0.0).size(g.x(cols), g.y(rows)).window_drag()
            .child(wallpaper::pane(ui, "signin", Rect::new(0, 0, cols, rows), caption, hints)));
    if let Some(form) = session.form() {
        root = root.child(dialog(ui, form, matches!(session, Session::SigningIn(_)), relay, cols, rows));
    }
    root
}

/// The form's rows, top to bottom, each with its key.
fn rows(ui: &Ui, form: &Form, sending: bool, relay: &str, width: usize) -> Vec<(Line, &'static str)> {
    let p = &ui.p;
    let inner = width.saturating_sub(4);
    let text = |s: &str, ink, key| (Line::new().gap(2).put(&grid::clip(s, inner), ink), key);
    let field = |label: &str, shown: &str, which: Field, key| {
        let focused = !sending && form.field == which;
        let value_cols = inner.saturating_sub(LABEL_COLS + 2);
        let mut line = Line::new().gap(2).put(&grid::fit(label, LABEL_COLS), p.header_key).put(&grid::clip(shown, value_cols), if sending { p.muted } else { p.text });
        if focused { line = line.mark(Mark::Caret, p.accent).fill(1, width - 1, p.selection); }
        (line, key)
    };
    let offer = form.offer;
    let mut out = vec![
        (Line::new().gap(2).bold(&grid::clip("Sign in to the relay", inner), p.text), "signin-title"),
        text(relay, p.muted, "signin-relay"),
        text("", p.muted, "signin-gap-1"),
    ];
    if offer.password {
        out.push(field("user", &form.user, Field::User, "signin-user"));
        out.push(field("password", &form.password.masked(), Field::Password, "signin-password"));
    }
    if offer.oidc {
        if offer.password { out.push(text("", p.muted, "signin-gap-2")); }
        out.push(field("", "sign in with your identity provider", Field::Oidc, "signin-oidc"));
    }
    if offer.token {
        if offer.password || offer.oidc {
            out.push(text("", p.muted, "signin-gap-3"));
            out.push(text("or, on a single-user relay, its token", p.muted, "signin-or"));
        }
        out.push(field("token", &form.token.masked(), Field::Token, "signin-token"));
    }
    out.push(text("", p.muted, "signin-gap-4"));
    match (&form.error, sending) {
        (_, true) => out.push(text("signing in", p.muted, "signin-state")),
        (Some(error), false) => out.push(text(error, p.error, "signin-error")),
        (None, false) if offer.token => out.push(text("The token is kept for this tab only.", p.muted, "signin-note")),
        (None, false) => out.push(text("The relay keeps the session; this page never sees it.", p.muted, "signin-note")),
    }
    out.push(text("tab next field    enter sign in", p.muted, "signin-hints"));
    out
}

fn dialog(ui: &Ui, form: &Form, sending: bool, relay: &str, cols: usize, rows_total: usize) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let width = MAX_COLS.min(cols.saturating_sub(4)).max(24);
    let lines = rows(ui, form, sending, relay, width);
    let height = (lines.len() + 2).min(rows_total.saturating_sub(2)).max(3);
    let (x, y) = (cols.saturating_sub(width) / 2, (rows_total.saturating_sub(height) / 2).max(1));
    let mut dialog = el().key("signin").role("dialog").label("Sign in to the relay").absolute(g.x(x), g.y(y)).size(g.x(width), g.y(height)).fill(p.bg).border(1.0, p.accent);
    for (i, (line, key)) in lines.into_iter().enumerate().take(height.saturating_sub(2)) {
        dialog = dialog.child(line.el(ui, width).absolute(0.0, g.y(i + 1)).key(key));
    }
    dialog
}

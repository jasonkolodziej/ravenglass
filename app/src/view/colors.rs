//! The `:colors` pane (architecture §11 "Contrast guardrail"): every palette name with its
//! swatch, and every role with the palette name it points at and its contrast against the
//! background. Text roles under 4.5:1 are marked; `:theme fix-contrast` nudges them.

use super::line::Line;
use super::{Ui, statusline};
use crate::grid::{self, Rect};
use crate::settings::MIN_CONTRAST;
use crate::state::AppState;
use ravenglass_theme::{Appearance, NAMES, Rgb};
use rune::{Color, El, el};

const PAD: usize = 2;
/// Columns of the palette half: swatch, name, hex.
const PALETTE_COLS: usize = 34;
const FOOT: &str = "esc close    :theme fix-contrast    :theme save <name>    :theme import <file>";

fn ink(c: Rgb) -> Color { rune::color::hex(&c.hex()).unwrap_or(Color::WHITE) }

pub fn pane(s: &AppState, ui: &Ui, area: Rect) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let theme = &s.theme;
    let failing: Vec<String> = theme.contrast_report(MIN_CONTRAST).into_iter().map(|(role, _)| role).collect();
    let appearance = match theme.appearance { Appearance::Dark => "dark", Appearance::Light => "light" };
    let rows = area.h.saturating_sub(1);
    let mut e = ui.place(el().key("colors").role("region").label("Colours and contrast"), area);
    let put = |e: El, row: usize, col: usize, line: Line, width: usize, key: Option<String>| {
        if row >= rows || col + width > area.w { return e; }
        let mut line = line.el(ui, width).absolute(g.x(col), g.y(row));
        if let Some(key) = key { line = line.key(key); }
        e.child(line)
    };

    let title = format!("{} · {} · {appearance}", theme.name, theme.id);
    e = put(e, 1, PAD, Line::new().bold(&grid::clip(&title, area.w.saturating_sub(2 * PAD)), p.text), area.w.saturating_sub(2 * PAD), Some("colors-title".into()));
    let summary = if failing.is_empty() { "every text role passes 4.5:1".to_owned() } else { format!("{} text roles under 4.5:1", failing.len()) };
    e = put(e, 2, PAD, Line::new().put(&summary, if failing.is_empty() { p.muted } else { p.warn }), area.w.saturating_sub(2 * PAD), Some("colors-summary".into()));

    e = put(e, 4, PAD, Line::new().put("palette", p.header_key), PALETTE_COLS, None);
    for (i, name) in NAMES.iter().enumerate() {
        let colour = theme.color(name);
        // The swatch is the palette colour itself: that is what this pane is for.
        let line = Line::new().gap(4).gap(2).put(&grid::fit(name, 12), p.text).gap(1).put(&colour.hex(), p.muted).fill(0, 4, ink(colour));
        e = put(e, 5 + i, PAD, line, PALETTE_COLS, Some(format!("swatch:{name}")));
    }

    let col = PAD + PALETTE_COLS + 2;
    let width = area.w.saturating_sub(col + PAD).min(60);
    if width >= 36 {
        e = put(e, 4, col, Line::new().put("role", p.header_key).to(20).put("colour", p.header_key).to(36).put("contrast", p.header_key), width, None);
        for (i, (role, name, ratio)) in s.role_contrasts().into_iter().enumerate() {
            let low = failing.contains(&role);
            let mut line = Line::new().put(&grid::fit(&role, 19), p.text).gap(4).put(&grid::fit(&name, 11), p.muted).gap(1).fill(20, 22, ink(theme.color(&name)));
            line = line.put(&grid::fit_right(&format!("{ratio:.1}:1"), 8), if low { p.warn } else { p.muted });
            if low { line = line.gap(2).put("low", p.warn); }
            e = put(e, 5 + i, col, line, width, Some(format!("role:{role}")));
        }
    }
    if rows > 2 && grid::width(FOOT) + 2 * PAD <= area.w {
        e = put(e, rows - 1, PAD, Line::new().put(FOOT, p.muted), grid::width(FOOT), None);
    }
    if area.h > 3 { e = e.child(statusline::line(s, ui, area.w).absolute(0.0, g.y(area.h - 1))); }
    e
}

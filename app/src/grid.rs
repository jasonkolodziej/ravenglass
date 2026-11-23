//! The character grid (plan §4 "The grid"): one cell is the advance of `0` in the UI mono font by
//! the line height, both rounded to device pixels. Every size the views use is a whole number of
//! cells; text is padded and cut in code, never by flex.

use rune::tok::TextStyle;

/// Cell metrics for one font size at one device scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub cell_w: f32,
    pub cell_h: f32,
    pub font_size: f32,
    /// Letter spacing that makes the font's advance exactly one cell.
    pub tracking: f32,
}

const PROBE: &str = "0000000000";

impl Grid {
    /// Pure: `advance` is the measured width of one `0` in points.
    pub fn from_metrics(font_size: f32, line_height: f32, advance: f32, scale: f32) -> Grid {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let snap = |v: f32| ((v * scale).round().max(1.0)) / scale;
        let cell_w = snap(advance);
        Grid { cell_w, cell_h: snap(font_size * line_height), font_size, tracking: cell_w - advance }
    }

    /// Measures the mono font the window draws with (IBM Plex Mono unless the theme says otherwise).
    pub fn measure(font_size: f32, line_height: f32, scale: f32) -> Grid {
        let advance = rune::text::advance(PROBE, TextStyle::new(font_size, 400.0).mono()) / PROBE.len() as f32;
        let advance = if advance.is_finite() && advance > 0.0 { advance } else { font_size * 0.6 };
        Grid::from_metrics(font_size, line_height, advance, scale)
    }

    pub fn x(&self, cols: usize) -> f32 { cols as f32 * self.cell_w }
    pub fn y(&self, rows: usize) -> f32 { rows as f32 * self.cell_h }
    /// Whole cells that fit in `points`.
    pub fn cols_in(&self, points: f32) -> usize { (points / self.cell_w + 0.01).floor().max(0.0) as usize }
    pub fn rows_in(&self, points: f32) -> usize { (points / self.cell_h + 0.01).floor().max(0.0) as usize }

    /// The text style of one grid line: mono, one cell per character, one cell tall.
    pub fn style(&self, weight: f32) -> TextStyle {
        TextStyle::new(self.font_size, weight).mono().lh(self.cell_h).tracking(self.tracking)
    }
}

/// A rectangle in cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect { pub x: usize, pub y: usize, pub w: usize, pub h: usize }

impl Rect {
    pub const fn new(x: usize, y: usize, w: usize, h: usize) -> Rect { Rect { x, y, w, h } }
    pub fn right(&self) -> usize { self.x + self.w }
    pub fn bottom(&self) -> usize { self.y + self.h }
    pub fn is_empty(&self) -> bool { self.w == 0 || self.h == 0 }
}

/// Number of cells `s` takes (one per character; wide glyphs are not handled yet).
pub fn width(s: &str) -> usize { s.chars().count() }

/// `s` cut to at most `n` cells, with `…` when it was cut.
pub fn clip(s: &str, n: usize) -> String {
    if width(s) <= n { return s.to_owned(); }
    if n == 0 { return String::new(); }
    let mut out: String = s.chars().take(n - 1).collect();
    out.push('…');
    out
}

/// `s` in exactly `n` cells: cut with `…` or padded with spaces on the right.
pub fn fit(s: &str, n: usize) -> String {
    let mut out = clip(s, n);
    for _ in width(&out)..n { out.push(' '); }
    out
}

/// `s` in exactly `n` cells, right-aligned.
pub fn fit_right(s: &str, n: usize) -> String {
    let cut = clip(s, n);
    let mut out = " ".repeat(n - width(&cut));
    out.push_str(&cut);
    out
}

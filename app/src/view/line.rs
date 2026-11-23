//! One line of the character grid: a run of cells built left to right, drawn as a single shaped
//! text with colour spans, cell-aligned background fills under it and vector marks over it for
//! the glyphs the font does not have (the unread dot, flag, attachment and sync marks).

use super::Ui;
use crate::grid::{self, Grid};
use rune::shape::LineCap;
use rune::text::Span;
use rune::{Color, El, Shape, canvas, el, rich};

/// A glyph drawn as a shape in one cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// The unread dot.
    Dot,
    Flag,
    /// Attachment: two stacked sheets.
    Clip,
    /// The sync arrow.
    Sync,
    /// The text caret: a full block.
    Caret,
}

#[derive(Default)]
pub struct Line {
    text: String,
    spans: Vec<Span>,
    fills: Vec<(usize, usize, Color)>,
    marks: Vec<(usize, Mark, Color)>,
    cols: usize,
}

impl Line {
    pub fn new() -> Line { Line::default() }

    /// Cells used so far.
    pub fn cols(&self) -> usize { self.cols }

    fn push(&mut self, s: &str, color: Color, weight: Option<f32>, underline: bool) {
        if s.is_empty() { return; }
        let start = self.text.len();
        self.text.push_str(s);
        self.cols += grid::width(s);
        let mut span = Span::new(start, self.text.len()).color(color);
        if let Some(w) = weight { span = span.weight(w); }
        if underline { span = span.underline(color); }
        self.spans.push(span);
    }

    pub fn put(mut self, s: &str, color: Color) -> Line {
        self.push(s, color, None, false);
        self
    }

    pub fn bold(mut self, s: &str, color: Color) -> Line {
        self.push(s, color, Some(super::BOLD), false);
        self
    }

    pub fn link(mut self, s: &str, color: Color) -> Line {
        self.push(s, color, None, true);
        self
    }

    pub fn gap(mut self, n: usize) -> Line {
        self.text.extend(std::iter::repeat_n(' ', n));
        self.cols += n;
        self
    }

    /// Pad with spaces up to column `col`.
    pub fn to(self, col: usize) -> Line {
        let n = col.saturating_sub(self.cols);
        self.gap(n)
    }

    /// Text on a filled run of cells, one cell of padding each side (the mode block).
    pub fn block(mut self, s: &str, fg: Color, bg: Color) -> Line {
        let start = self.cols;
        self = self.gap(1);
        self.push(s, fg, Some(super::BOLD), false);
        self = self.gap(1);
        self.fills.push((start, self.cols, bg));
        self
    }

    /// Fill cells `from..to` under the text.
    pub fn fill(mut self, from: usize, to: usize, color: Color) -> Line {
        if to > from { self.fills.push((from, to, color)); }
        self
    }

    /// A drawn glyph in the next cell.
    pub fn mark(mut self, mark: Mark, color: Color) -> Line {
        self.marks.push((self.cols, mark, color));
        self.gap(1)
    }

    /// The line as an element `width` cells wide and one cell tall. Anything past `width` is cut.
    pub fn el(mut self, ui: &Ui, width: usize) -> El {
        let g = &ui.g;
        if self.cols > width {
            let end = self.text.char_indices().nth(width).map_or(self.text.len(), |(i, _)| i);
            self.text.truncate(end);
            self.spans.retain_mut(|s| {
                s.end = s.end.min(end as u32);
                s.start < s.end
            });
        }
        let mut line = el().size(g.x(width), g.cell_h);
        for (from, to, color) in self.fills {
            let to = to.min(width);
            if to > from { line = line.child(el().absolute(g.x(from), 0.0).size(g.x(to - from), g.cell_h).fill(color)); }
        }
        if !self.text.trim().is_empty() {
            line = line.child(rich(self.text, self.spans).font(g.style(400.0)).color(ui.p.text).absolute(0.0, 0.0));
        }
        for (col, mark, color) in self.marks {
            if col < width { line = line.child(mark_el(g, mark, color).absolute(g.x(col), 0.0)); }
        }
        line
    }
}

/// A mark drawn in one cell, in the cell's own points.
pub fn mark_el(g: &Grid, mark: Mark, color: Color) -> El {
    let (w, h) = (g.cell_w, g.cell_h);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let shape = match mark {
        Mark::Dot => {
            let r = (w * 0.5).min(h * 0.25);
            Shape::new().fill(circle(cx, cy, r), color)
        }
        Mark::Flag => {
            let (x0, top, mid) = (w * 0.2, h * 0.24, h * 0.52);
            Shape::new()
                .fill(vec![[x0, top], [w * 0.95, top], [w * 0.7, (top + mid) / 2.0], [w * 0.95, mid], [x0, mid]], color)
                .line([x0, top], [x0, h * 0.78], 1.2, LineCap::Butt, color)
        }
        Mark::Clip => {
            let (a, b) = (w * 0.34, h * 0.2);
            let back = vec![[w * 0.08, h * 0.28], [w * 0.62, h * 0.28], [w * 0.62, h * 0.66], [w * 0.08, h * 0.66]];
            let front = vec![[a, h * 0.4], [w * 0.94, h * 0.4], [w * 0.94, h * 0.4 + b * 1.9], [a, h * 0.4 + b * 1.9]];
            Shape::new().stroke_with(back, true, 1.0, LineCap::Butt, color).stroke_with(front, true, 1.0, LineCap::Butt, color)
        }
        Mark::Sync => {
            let r = w * 0.44;
            // Three quarters of a circle, open at the top right, with an arrow head on its end.
            let arc: Vec<[f32; 2]> = (0..=18).map(|i| {
                let a = (-50.0 + 290.0 * i as f32 / 18.0).to_radians();
                [cx + r * a.cos(), cy + r * a.sin()]
            }).collect();
            let tip = arc[0];
            Shape::new().stroke_with(arc, false, 1.1, LineCap::Round, color).fill(vec![[tip[0] - 2.6, tip[1] - 2.2], [tip[0] + 1.6, tip[1] - 2.6], [tip[0] + 1.0, tip[1] + 2.0]], color)
        }
        Mark::Caret => Shape::new().fill(vec![[0.0, h * 0.12], [w, h * 0.12], [w, h * 0.9], [0.0, h * 0.9]], color),
    };
    canvas(shape).size(w, h).inert()
}

fn circle(cx: f32, cy: f32, r: f32) -> Vec<[f32; 2]> {
    (0..16).map(|i| {
        let a = i as f32 / 16.0 * std::f32::consts::TAU;
        [cx + r * a.cos(), cy + r * a.sin()]
    }).collect()
}

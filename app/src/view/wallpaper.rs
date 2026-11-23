//! The wallpaper of a pane with nothing to show (plan §4 "Brand"): the braille raven, sized to
//! the pane and drawn as one canvas shape (a capsule per dot on the 2×4-dots-per-cell grid),
//! with the wordmark, a caption and a key-hint line under it.

use super::Ui;
use super::line::Line;
use crate::actions::Action;
use crate::grid::{self, Rect};
use crate::keys;
use ravenglass_brand::{Dot, FieldSpec, cols_for, dot_field};
use rune::shape::{LineCap, Shape};
use rune::{El, canvas, el};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// The reference rendering's height in rows; panes that can't fit it get a smaller raven.
const FULL_ROWS: u32 = 26;
const MIN_ROWS: u32 = 8;
/// Below this the facet gaps close up, so they are left out.
const FACET_ROWS: u32 = 16;
/// Rows under the raven: a gap, the wordmark, the caption, the hints.
const TEXT_ROWS: usize = 5;

thread_local! {
    /// Dot fields by height. At most `FULL_ROWS - MIN_ROWS + 1` entries, a few thousand dots each.
    static FIELDS: RefCell<BTreeMap<u32, Rc<Vec<Dot>>>> = const { RefCell::new(BTreeMap::new()) };
}

fn field(rows: u32) -> Rc<Vec<Dot>> {
    FIELDS.with_borrow_mut(|f| f.entry(rows).or_insert_with(|| Rc::new(dot_field(FieldSpec { rows, facets: rows >= FACET_ROWS }))).clone())
}

/// The key-hint line, from the key table.
pub fn hints() -> String {
    format!("/ search    {} commands    1–9 workspaces    c compose", keys::hint(&Action::Palette))
}

/// The largest raven height (in rows) that fits `area` with its text.
pub fn raven_rows(area: Rect) -> Option<u32> {
    let room = area.h.saturating_sub(TEXT_ROWS + 2) as u32;
    let mut rows = room.min(FULL_ROWS);
    while rows >= MIN_ROWS && cols_for(rows) as usize + 4 > area.w { rows -= 1; }
    (rows >= MIN_ROWS).then_some(rows)
}

/// `id` names the pane, so each wallpaper keeps its own memoized raven.
pub fn pane(ui: &Ui, id: &str, area: Rect, caption: &str, hints: &str) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let mut e = el().key("wallpaper").size(g.x(area.w), g.y(area.h)).inert();
    let raven = raven_rows(area);
    let raven_h = raven.map_or(0, |r| r as usize + 1);
    let top = area.h.saturating_sub(raven_h + TEXT_ROWS - 1) / 2;
    if let Some(rows) = raven {
        let cols = cols_for(rows) as usize;
        let x = area.w.saturating_sub(cols) / 2;
        let (ink, eye, opacity) = (p.wallpaper, p.accent, ui.wallpaper_opacity);
        // A few thousand dots: built once per size and theme, not on every view run.
        let version = (rows, x, top, g.cell_w.to_bits(), g.cell_h.to_bits(), ink.hex(), eye.hex(), opacity.to_bits());
        let g = *g;
        e = e.child(rune::memo(format!("raven:{id}"), version, move || {
            let (pitch_x, pitch_y) = (g.cell_w / 2.0, g.cell_h / 4.0);
            let d = pitch_x.min(pitch_y) * 0.56;
            let mut shape = Shape::new();
            for dot in field(rows).iter() {
                let (x, y) = ((dot.x as f32 + 0.5) * pitch_x, (dot.y as f32 + 0.5) * pitch_y);
                shape = shape.line([x - 0.01, y], [x + 0.01, y], d, LineCap::Round, if dot.eye { eye } else { ink });
            }
            canvas(shape).key("raven").absolute(g.x(x), g.y(top)).size(g.x(cols), g.y(rows as usize)).opacity(opacity).inert()
        }));
    }
    let centred = |text: &str, row: usize, line: Line| {
        let width = grid::width(text).min(area.w);
        line.el(ui, width).absolute(g.x(area.w.saturating_sub(width) / 2), g.y(top + raven_h + row))
    };
    const WORDMARK: &str = "R A V E N G L A S S";
    if area.h >= 3 {
        e = e.child(centred(WORDMARK, 1, Line::new().bold(WORDMARK, p.text)));
        e = e.child(centred(caption, 2, Line::new().put(caption, p.muted)));
    }
    if area.h >= TEXT_ROWS && grid::width(hints) <= area.w {
        e = e.child(centred(hints, 3, Line::new().put(hints, p.muted)));
    }
    e
}

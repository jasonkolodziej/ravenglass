//! The raven mark as data, and the braille dot field the wallpaper and launch intro draw
//! (plan §4 "Brand"). The app turns each [`Dot`] into a small Rune circle; the intro springs dots
//! from scattered start positions to these targets.

pub mod geometry;

use geometry::*;

/// One braille dot, in *dot* coordinates: x in `0..cols*2`, y in `0..rows*4`.
/// Multiply by the cell size / (2, 4) to place it in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot { pub x: u32, pub y: u32, pub eye: bool }

#[derive(Clone, Copy, Debug)]
pub struct FieldSpec {
    /// Height in text rows; width follows the raven's aspect (cells are 1:2, dots are square).
    pub rows: u32,
    /// Leave gaps along facet edges (the "cut glass" look). Off for very small sizes.
    pub facets: bool,
}

/// Columns needed for `rows`, keeping the raven's aspect with square dots.
pub fn cols_for(rows: u32) -> u32 {
    let h = (rows * 4) as f32;
    ((h * VIEW_BOX[2] / VIEW_BOX[3]) / 2.0).round() as u32
}

/// The dot field: every dot inside the silhouette, minus a thin gap along facet edges, minus the eye.
pub fn dot_field(spec: FieldSpec) -> Vec<Dot> {
    let (w, h) = (cols_for(spec.rows) * 2, spec.rows * 4);
    let unit = VIEW_BOX[3] / h as f32; // design units per dot
    let gap = unit * 0.55;
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let p = [VIEW_BOX[0] + (x as f32 + 0.5) * unit, VIEW_BOX[1] + (y as f32 + 0.5) * unit];
            if !inside(p, &OUTLINE) { continue; }
            let d_eye = dist(p, EYE);
            if d_eye < unit * 1.2 { out.push(Dot { x, y, eye: true }); continue; }
            if spec.facets && EDGES.iter().any(|[a, b]| seg_dist(p, POINTS[*a], POINTS[*b]) < gap) { continue; }
            out.push(Dot { x, y, eye: false });
        }
    }
    out
}

/// The same field as braille text (for terminals, tests and the ASCII wallpaper option).
pub fn braille_text(spec: FieldSpec) -> String {
    let (cols, rows) = (cols_for(spec.rows) as usize, spec.rows as usize);
    let mut cells = vec![0u8; cols * rows];
    for d in dot_field(spec) {
        let (cx, cy) = ((d.x / 2) as usize, (d.y / 4) as usize);
        if cx >= cols || cy >= rows || d.eye { continue; }
        const BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
        cells[cy * cols + cx] |= BITS[(d.y % 4) as usize][(d.x % 2) as usize];
    }
    cells.chunks(cols).map(|r| r.iter().map(|&b| char::from_u32(0x2800 + b as u32).unwrap()).collect::<String>().trim_end_matches('\u{2800}').to_owned())
        .collect::<Vec<_>>().join("\n")
}

fn inside(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] { c = !c; }
        j = i;
    }
    c
}
fn dist(a: [f32; 2], b: [f32; 2]) -> f32 { ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() }
fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_is_deterministic_and_dense() {
        let s = FieldSpec { rows: 26, facets: true };
        let a = dot_field(s);
        assert_eq!(a, dot_field(s));
        assert!(a.len() > 3000, "{} dots", a.len());
        assert!(a.iter().any(|d| d.eye));
    }

    #[test]
    fn facets_remove_dots() {
        let on = dot_field(FieldSpec { rows: 26, facets: true }).len();
        let off = dot_field(FieldSpec { rows: 26, facets: false }).len();
        assert!(off > on + 200, "facet gaps: {off} vs {on}");
    }

    #[test]
    fn braille_text_has_the_expected_shape() {
        let t = braille_text(FieldSpec { rows: 26, facets: true });
        assert_eq!(t.split('\n').count(), 26);
        assert_eq!(cols_for(26), 53);
        assert!(t.lines().map(|l| l.chars().count()).max().unwrap() <= 53);
    }
}

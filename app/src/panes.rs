//! The pane split tree (plan §4 "Layout"): panes are the layout primitive. Every size here is in
//! whole cells; a split keeps a ratio so the layout follows window resizes, and resizing a pane
//! moves the divider by whole cells.

use crate::grid::Rect;

pub type PaneId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    List,
    Reader,
    /// Nothing to show: draws the wallpaper.
    Empty,
    /// The draft being written (`compose.rs`).
    Compose,
}

/// How a split lays its two children out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    /// Side by side, divided by a one-column gutter (vim's `:vsplit`).
    Cols,
    /// Stacked, divided by a one-row gutter (vim's `:split`).
    Rows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir { Left, Right, Up, Down }

impl Dir {
    fn axis(self) -> Axis { if matches!(self, Dir::Left | Dir::Right) { Axis::Cols } else { Axis::Rows } }
}

/// The smallest pane a split leaves, in cells.
pub const MIN_COLS: usize = 12;
pub const MIN_ROWS: usize = 4;
/// Cells between two panes.
pub const GUTTER: usize = 1;

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Leaf { id: PaneId, kind: Kind },
    Split { axis: Axis, ratio: f32, a: Box<Node>, b: Box<Node> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed { pub id: PaneId, pub kind: Kind, pub rect: Rect }

/// A divider between two panes: `rect` is the gutter's cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gutter { pub axis: Axis, pub rect: Rect }

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Layout { pub panes: Vec<Placed>, pub gutters: Vec<Gutter> }

impl Layout {
    pub fn pane(&self, id: PaneId) -> Option<&Placed> { self.panes.iter().find(|p| p.id == id) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaneTree {
    root: Node,
    focus: PaneId,
    next_id: PaneId,
}

/// Why a pane operation did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneError { LastPane, TooSmall, NoNeighbour, NoSplit }

impl PaneError {
    pub fn message(self) -> &'static str {
        match self {
            PaneError::LastPane => "can't close the last pane",
            PaneError::TooSmall => "not enough room to split",
            PaneError::NoNeighbour => "no pane that way",
            PaneError::NoSplit => "nothing to resize against",
        }
    }
}

impl PaneTree {
    pub fn single(kind: Kind) -> PaneTree { PaneTree { root: Node::Leaf { id: 1, kind }, focus: 1, next_id: 2 } }

    /// The default workspace: the message list on the left, the reader on the right.
    pub fn mail() -> PaneTree {
        let root = Node::Split {
            axis: Axis::Cols,
            ratio: 0.5,
            a: Box::new(Node::Leaf { id: 1, kind: Kind::List }),
            b: Box::new(Node::Leaf { id: 2, kind: Kind::Reader }),
        };
        PaneTree { root, focus: 1, next_id: 3 }
    }

    pub fn focus(&self) -> PaneId { self.focus }

    pub fn focused_kind(&self) -> Kind { self.kind_of(self.focus).unwrap_or(Kind::Empty) }

    pub fn kind_of(&self, id: PaneId) -> Option<Kind> { self.leaves().into_iter().find(|(i, _)| *i == id).map(|(_, k)| k) }

    /// Every pane, in reading order.
    pub fn leaves(&self) -> Vec<(PaneId, Kind)> {
        fn walk(n: &Node, out: &mut Vec<(PaneId, Kind)>) {
            match n {
                Node::Leaf { id, kind } => out.push((*id, *kind)),
                Node::Split { a, b, .. } => { walk(a, out); walk(b, out); }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    pub fn find(&self, kind: Kind) -> Option<PaneId> { self.leaves().into_iter().find(|(_, k)| *k == kind).map(|(id, _)| id) }

    pub fn set_focus(&mut self, id: PaneId) -> bool {
        let known = self.kind_of(id).is_some();
        if known { self.focus = id; }
        known
    }

    /// Focus the first pane of `kind`.
    pub fn focus_kind(&mut self, kind: Kind) -> bool {
        match self.find(kind) {
            Some(id) => { self.focus = id; true }
            None => false,
        }
    }

    pub fn set_kind(&mut self, id: PaneId, kind: Kind) {
        fn walk(n: &mut Node, target: PaneId, to: Kind) {
            match n {
                Node::Leaf { id, kind } => if *id == target { *kind = to; },
                Node::Split { a, b, .. } => { walk(a, target, to); walk(b, target, to); }
            }
        }
        walk(&mut self.root, id, kind);
    }

    /// Where every pane and gutter sits inside `area`.
    pub fn layout(&self, area: Rect) -> Layout {
        let mut out = Layout::default();
        place(&self.root, area, &mut out);
        out
    }

    /// Narrow windows collapse to the focused pane alone.
    pub fn layout_single(&self, area: Rect) -> Layout {
        Layout { panes: vec![Placed { id: self.focus, kind: self.focused_kind(), rect: area }], gutters: Vec::new() }
    }

    /// Split the focused pane; the new pane takes the far half, shows `kind` and takes focus.
    pub fn split(&mut self, axis: Axis, kind: Kind, area: Rect) -> Result<PaneId, PaneError> {
        let rect = self.layout(area).pane(self.focus).map(|p| p.rect).ok_or(PaneError::TooSmall)?;
        let (have, min) = match axis { Axis::Cols => (rect.w, MIN_COLS), Axis::Rows => (rect.h, MIN_ROWS) };
        if have < 2 * min + GUTTER { return Err(PaneError::TooSmall); }
        let new = self.next_id;
        self.next_id += 1;
        fn walk(n: &mut Node, target: PaneId, axis: Axis, new: PaneId, kind: Kind) {
            match n {
                Node::Leaf { id, .. } if *id == target => {
                    let old = std::mem::replace(n, Node::Leaf { id: new, kind });
                    *n = Node::Split { axis, ratio: 0.5, a: Box::new(old), b: Box::new(Node::Leaf { id: new, kind }) };
                }
                Node::Leaf { .. } => {}
                Node::Split { a, b, .. } => { walk(a, target, axis, new, kind); walk(b, target, axis, new, kind); }
            }
        }
        walk(&mut self.root, self.focus, axis, new, kind);
        self.focus = new;
        Ok(new)
    }

    /// Close the focused pane; its sibling takes the room and the focus.
    pub fn close(&mut self) -> Result<(), PaneError> {
        if matches!(self.root, Node::Leaf { .. }) { return Err(PaneError::LastPane); }
        fn first(n: &Node) -> PaneId {
            match n { Node::Leaf { id, .. } => *id, Node::Split { a, .. } => first(a) }
        }
        fn walk(n: &mut Node, target: PaneId) -> Option<PaneId> {
            let Node::Split { a, b, .. } = n else { return None };
            let keep = match (&**a, &**b) {
                (Node::Leaf { id, .. }, _) if *id == target => Some(b.clone()),
                (_, Node::Leaf { id, .. }) if *id == target => Some(a.clone()),
                _ => None,
            };
            if let Some(keep) = keep {
                *n = *keep;
                return Some(first(n));
            }
            walk(a, target).or_else(|| walk(b, target))
        }
        match walk(&mut self.root, self.focus) {
            Some(next) => { self.focus = next; Ok(()) }
            None => Err(PaneError::LastPane),
        }
    }

    /// Move focus to the pane next to the focused one in `dir`.
    pub fn focus_dir(&mut self, dir: Dir, area: Rect) -> Result<PaneId, PaneError> {
        let layout = self.layout(area);
        let from = layout.pane(self.focus).ok_or(PaneError::NoNeighbour)?.rect;
        let overlap = |a0: usize, a1: usize, b0: usize, b1: usize| a1.min(b1).saturating_sub(a0.max(b0));
        let best = layout.panes.iter().filter(|p| p.id != self.focus).filter_map(|p| {
            let r = p.rect;
            let (adjacent, shared) = match dir {
                Dir::Left => (r.right() + GUTTER == from.x, overlap(r.y, r.bottom(), from.y, from.bottom())),
                Dir::Right => (from.right() + GUTTER == r.x, overlap(r.y, r.bottom(), from.y, from.bottom())),
                Dir::Up => (r.bottom() + GUTTER == from.y, overlap(r.x, r.right(), from.x, from.right())),
                Dir::Down => (from.bottom() + GUTTER == r.y, overlap(r.x, r.right(), from.x, from.right())),
            };
            (adjacent && shared > 0).then_some((shared, p.id))
        }).max_by_key(|(shared, id)| (*shared, std::cmp::Reverse(*id)));
        match best {
            Some((_, id)) => { self.focus = id; Ok(id) }
            None => Err(PaneError::NoNeighbour),
        }
    }

    /// Move the divider nearest the focused pane by `cells` in `dir` (right and down are
    /// positive along their axis). The divider stops where either side would go under its minimum.
    pub fn resize(&mut self, dir: Dir, cells: usize, area: Rect) -> Result<(), PaneError> {
        let delta = if matches!(dir, Dir::Right | Dir::Down) { cells as isize } else { -(cells as isize) };
        let focus = self.focus;
        fn contains(n: &Node, target: PaneId) -> bool {
            match n { Node::Leaf { id, .. } => *id == target, Node::Split { a, b, .. } => contains(a, target) || contains(b, target) }
        }
        // The innermost split along the axis that holds the focused pane.
        fn walk(n: &mut Node, area: Rect, target: PaneId, want: Axis, delta: isize) -> bool {
            let Node::Split { axis, ratio, a, b } = n else { return false };
            let (ra, rb, _) = divide(*axis, *ratio, area);
            let (in_a, in_b) = (contains(a, target), contains(b, target));
            if in_a && walk(a, ra, target, want, delta) { return true; }
            if in_b && walk(b, rb, target, want, delta) { return true; }
            if *axis != want || !(in_a || in_b) { return false; }
            let (total, min, size_a) = match axis { Axis::Cols => (area.w, MIN_COLS, ra.w), Axis::Rows => (area.h, MIN_ROWS, ra.h) };
            if total < 2 * min + GUTTER { return false; }
            let free = total - GUTTER;
            let next = (size_a as isize + delta).clamp(min as isize, (free - min) as isize) as usize;
            if next == size_a { return false; }
            // Half a cell inside the target, so rounding lands on it at this size.
            *ratio = (next as f32) / (free as f32);
            true
        }
        if walk(&mut self.root, area, focus, dir.axis(), delta) { Ok(()) } else { Err(PaneError::NoSplit) }
    }

    /// Drag a gutter: put divider `index` (its place in [`Layout::gutters`]) on cell `cell`
    /// (a column for a column split, a row for a row split), within the panes' minimums.
    /// True when the divider moved.
    pub fn drag_gutter(&mut self, index: usize, cell: usize, area: Rect) -> bool {
        // Mirrors `place`: children first, then the split's own gutter.
        fn walk(n: &mut Node, area: Rect, seen: &mut usize, index: usize, cell: usize) -> Option<bool> {
            if area.is_empty() { return None; }
            let Node::Split { axis, ratio, a, b } = n else { return None };
            let (ra, rb, gutter) = divide(*axis, *ratio, area);
            if let Some(moved) = walk(a, ra, seen, index, cell) { return Some(moved); }
            if let Some(moved) = walk(b, rb, seen, index, cell) { return Some(moved); }
            gutter?;
            if *seen != index {
                *seen += 1;
                return None;
            }
            let (origin, total, min, size_a) = match axis { Axis::Cols => (area.x, area.w, MIN_COLS, ra.w), Axis::Rows => (area.y, area.h, MIN_ROWS, ra.h) };
            let free = total - GUTTER;
            let next = cell.saturating_sub(origin).clamp(min, free - min);
            if next != size_a { *ratio = next as f32 / free as f32; }
            Some(next != size_a)
        }
        walk(&mut self.root, area, &mut 0, index, cell).unwrap_or(false)
    }
}

/// The two halves of `area` and the gutter between them. Sizes are whole cells; when the area is
/// too small for two panes the first child takes it all.
fn divide(axis: Axis, ratio: f32, area: Rect) -> (Rect, Rect, Option<Rect>) {
    let (total, min) = match axis { Axis::Cols => (area.w, MIN_COLS), Axis::Rows => (area.h, MIN_ROWS) };
    if total < 2 * min + GUTTER {
        return (area, Rect::new(area.x, area.y, 0, 0), None);
    }
    let free = total - GUTTER;
    let a = ((free as f32 * ratio).round() as usize).clamp(min, free - min);
    match axis {
        Axis::Cols => (
            Rect::new(area.x, area.y, a, area.h),
            Rect::new(area.x + a + GUTTER, area.y, free - a, area.h),
            Some(Rect::new(area.x + a, area.y, GUTTER, area.h)),
        ),
        Axis::Rows => (
            Rect::new(area.x, area.y, area.w, a),
            Rect::new(area.x, area.y + a + GUTTER, area.w, free - a),
            Some(Rect::new(area.x, area.y + a, area.w, GUTTER)),
        ),
    }
}

fn place(n: &Node, area: Rect, out: &mut Layout) {
    if area.is_empty() { return; }
    match n {
        Node::Leaf { id, kind } => out.panes.push(Placed { id: *id, kind: *kind, rect: area }),
        Node::Split { axis, ratio, a, b } => {
            let (ra, rb, gutter) = divide(*axis, *ratio, area);
            place(a, ra, out);
            place(b, rb, out);
            if let Some(rect) = gutter { out.gutters.push(Gutter { axis: *axis, rect }); }
        }
    }
}

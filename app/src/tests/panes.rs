use crate::grid::Rect;
use crate::panes::*;

const AREA: Rect = Rect::new(0, 2, 159, 42);

fn rect(tree: &PaneTree, id: PaneId) -> Rect { tree.layout(AREA).pane(id).expect("pane is laid out").rect }

#[test]
fn the_default_layout_is_list_gutter_reader_in_whole_cells() {
    let tree = PaneTree::mail();
    let layout = tree.layout(AREA);
    assert_eq!(layout.panes.iter().map(|p| p.kind).collect::<Vec<_>>(), [Kind::List, Kind::Reader]);
    let (list, reader) = (layout.panes[0].rect, layout.panes[1].rect);
    assert_eq!(list, Rect::new(0, 2, 79, 42));
    assert_eq!(reader, Rect::new(80, 2, 79, 42));
    assert_eq!(layout.gutters, [Gutter { axis: Axis::Cols, rect: Rect::new(79, 2, 1, 42) }]);
    assert_eq!(list.w + GUTTER + reader.w, AREA.w, "the panes and the gutter fill the area exactly");
    assert_eq!(tree.focused_kind(), Kind::List);
}

#[test]
fn panes_tile_the_area_at_any_size() {
    let mut tree = PaneTree::mail();
    tree.split(Axis::Rows, Kind::Empty, AREA).unwrap();
    tree.set_focus(2);
    tree.split(Axis::Rows, Kind::Empty, AREA).unwrap();
    for (w, h) in [(159, 42), (120, 30), (80, 24), (61, 19), (200, 60)] {
        let area = Rect::new(0, 0, w, h);
        let layout = tree.layout(area);
        let mut cells = vec![0u8; w * h];
        for r in layout.panes.iter().map(|p| p.rect).chain(layout.gutters.iter().map(|g| g.rect)) {
            assert!(r.right() <= w && r.bottom() <= h, "{r:?} inside {w}x{h}");
            for y in r.y..r.bottom() {
                for x in r.x..r.right() { cells[y * w + x] += 1; }
            }
        }
        assert!(cells.iter().all(|c| *c == 1), "every cell of {w}x{h} belongs to exactly one pane or gutter");
        for p in &layout.panes {
            assert!(p.rect.w >= MIN_COLS && p.rect.h >= MIN_ROWS, "{:?} keeps the minimum at {w}x{h}", p.rect);
        }
    }
}

#[test]
fn a_split_halves_the_focused_pane_and_focuses_the_new_one() {
    let mut tree = PaneTree::mail();
    tree.set_focus(2);
    let new = tree.split(Axis::Rows, Kind::Empty, AREA).unwrap();
    assert_eq!(tree.focus(), new);
    assert_eq!(tree.leaves(), [(1, Kind::List), (2, Kind::Reader), (new, Kind::Empty)]);
    let (top, bottom) = (rect(&tree, 2), rect(&tree, new));
    assert_eq!((top.x, top.w), (bottom.x, bottom.w));
    assert_eq!(top.h + GUTTER + bottom.h, AREA.h);
    assert_eq!(bottom.y, top.bottom() + GUTTER);
    assert_eq!(rect(&tree, 1), Rect::new(0, 2, 79, 42), "the other side does not move");
}

#[test]
fn a_pane_too_small_to_halve_does_not_split() {
    let mut tree = PaneTree::mail();
    let small = Rect::new(0, 0, 40, 6);
    assert_eq!(tree.split(Axis::Rows, Kind::Empty, small), Err(PaneError::TooSmall));
    assert_eq!(tree.split(Axis::Cols, Kind::Empty, small), Err(PaneError::TooSmall));
    assert_eq!(tree.leaves().len(), 2);
}

#[test]
fn closing_gives_the_room_and_the_focus_to_the_sibling() {
    let mut tree = PaneTree::mail();
    tree.set_focus(2);
    let new = tree.split(Axis::Rows, Kind::Empty, AREA).unwrap();
    tree.close().unwrap();
    assert_eq!(tree.leaves(), [(1, Kind::List), (2, Kind::Reader)]);
    assert_eq!(tree.focus(), 2);
    assert!(tree.kind_of(new).is_none());
    assert_eq!(rect(&tree, 2), Rect::new(80, 2, 79, 42));
    tree.close().unwrap();
    assert_eq!(tree.leaves(), [(1, Kind::List)]);
    assert_eq!(rect(&tree, 1), AREA);
    assert_eq!(tree.close(), Err(PaneError::LastPane));
}

#[test]
fn focus_moves_to_the_neighbour_in_a_direction() {
    let mut tree = PaneTree::mail();
    tree.set_focus(2);
    let below = tree.split(Axis::Rows, Kind::Empty, AREA).unwrap();
    assert_eq!(tree.focus_dir(Dir::Up, AREA), Ok(2));
    assert_eq!(tree.focus_dir(Dir::Down, AREA), Ok(below));
    assert_eq!(tree.focus_dir(Dir::Left, AREA), Ok(1));
    assert_eq!(tree.focus_dir(Dir::Left, AREA), Err(PaneError::NoNeighbour));
    assert_eq!(tree.focus_dir(Dir::Up, AREA), Err(PaneError::NoNeighbour));
    // From the full-height list, right goes to the pane sharing the most rows; ties go to the first.
    assert_eq!(tree.focus_dir(Dir::Right, AREA), Ok(2));
}

#[test]
fn resizing_moves_the_divider_by_whole_cells() {
    let mut tree = PaneTree::mail();
    tree.resize(Dir::Right, 2, AREA).unwrap();
    assert_eq!((rect(&tree, 1).w, rect(&tree, 2).w), (81, 77));
    tree.resize(Dir::Left, 6, AREA).unwrap();
    assert_eq!((rect(&tree, 1).w, rect(&tree, 2).w), (75, 83));
    // Each step is exactly its cells, from either side of the divider.
    tree.set_focus(2);
    for step in 1..=5 {
        tree.resize(Dir::Left, 1, AREA).unwrap();
        assert_eq!(rect(&tree, 1).w, 75 - step);
    }
    assert_eq!(tree.resize(Dir::Down, 1, AREA), Err(PaneError::NoSplit), "no stacked split to resize");
}

#[test]
fn a_divider_stops_at_the_minimum_pane() {
    let mut tree = PaneTree::mail();
    tree.resize(Dir::Left, 500, AREA).unwrap();
    assert_eq!(rect(&tree, 1).w, MIN_COLS);
    assert_eq!(tree.resize(Dir::Left, 1, AREA), Err(PaneError::NoSplit));
    tree.resize(Dir::Right, 500, AREA).unwrap();
    assert_eq!(rect(&tree, 2).w, MIN_COLS);
}

#[test]
fn the_inner_split_resizes_before_the_outer_one() {
    let mut tree = PaneTree::mail();
    tree.set_focus(2);
    let below = tree.split(Axis::Rows, Kind::Empty, AREA).unwrap();
    let before = rect(&tree, 2).h;
    tree.resize(Dir::Down, 3, AREA).unwrap();
    assert_eq!(rect(&tree, 2).h, before + 3);
    assert_eq!(rect(&tree, below).h, AREA.h - GUTTER - before - 3);
    assert_eq!(rect(&tree, 1).w, 79, "the column split is untouched");
}

#[test]
fn the_ratio_follows_a_window_resize() {
    let mut tree = PaneTree::mail();
    tree.resize(Dir::Left, 19, AREA).unwrap();
    assert_eq!(rect(&tree, 1).w, 60);
    let wide = tree.layout(Rect::new(0, 0, 238, 50));
    assert_eq!(wide.panes[0].rect.w, 90, "60 of 158 free cells is 90 of 237");
}

#[test]
fn a_narrow_area_shows_the_focused_pane_alone() {
    let mut tree = PaneTree::mail();
    let area = Rect::new(0, 2, 20, 30);
    let collapsed = tree.layout(area);
    assert_eq!(collapsed.panes.len(), 1, "no room for two minimum panes: the first takes it all");
    tree.set_focus(2);
    let single = tree.layout_single(area);
    assert_eq!(single.panes, [Placed { id: 2, kind: Kind::Reader, rect: area }]);
    assert!(single.gutters.is_empty());
}

#[test]
fn kinds_can_be_found_changed_and_focused() {
    let mut tree = PaneTree::mail();
    assert_eq!(tree.find(Kind::Reader), Some(2));
    assert!(tree.focus_kind(Kind::Reader));
    assert_eq!(tree.focus(), 2);
    tree.set_kind(2, Kind::Empty);
    assert_eq!(tree.find(Kind::Reader), None);
    assert!(!tree.focus_kind(Kind::Reader));
    assert!(!tree.set_focus(99));
    assert_eq!(tree.focus(), 2);
}

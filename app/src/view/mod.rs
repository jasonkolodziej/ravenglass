//! Views: plain functions of `AppState`. Everything is placed in whole cells of the [`Grid`]
//! and every colour comes from a theme role through [`Paint`].

pub mod line;
pub mod list;
pub mod palette;
pub mod rail;
pub mod reader;
pub mod statusline;
pub mod tabs;
pub mod toast;
pub mod wallpaper;
pub mod workspaces;

use crate::grid::{Grid, Rect};
use crate::modes::Mode;
use crate::panes::{Axis, Gutter, Kind, Placed};
use crate::state::{AppState, Frame, NARROW_COLS};
use ravenglass_theme::Theme;
use rune::{Color, Cursor, Cx, Drag, El, el};
use std::rc::Rc;

/// The root element's key; its laid-out size is the window's.
pub const ROOT: &str = "rg-root";
/// The weight "bold" asks for. IBM Plex Mono ships Regular and Medium here; Rune picks the
/// nearest face.
pub const BOLD: f32 = 600.0;
/// Rows of the title bar and of the workspace bar.
pub const BAR_ROWS: usize = 2;
/// Columns of the icon rail.
pub const RAIL_COLS: usize = 5;

/// Theme colours by role, resolved once per view run.
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    pub bg: Color,
    pub text: Color,
    pub muted: Color,
    pub chrome: Color,
    pub border: Color,
    pub selection: Color,
    pub accent: Color,
    pub focus_bar: Color,
    pub brand: Color,
    pub on_accent: Color,
    pub mode: Color,
    pub unread: Color,
    pub flag: Color,
    pub attachment: Color,
    pub quote: Color,
    pub link: Color,
    pub header_key: Color,
    pub error: Color,
    pub wallpaper: Color,
}

impl Paint {
    pub fn of(theme: &Theme, mode: &Mode) -> Paint {
        let role = |name: &str| rune::color::hex(&theme.role(name).hex()).unwrap_or(Color::WHITE);
        Paint {
            bg: role("ui.bg"),
            text: role("ui.text"),
            muted: role("ui.muted"),
            chrome: role("ui.chrome"),
            border: role("ui.border"),
            selection: role("ui.selection"),
            accent: role("ui.accent"),
            focus_bar: role("ui.focusBar"),
            brand: role("ui.brand"),
            on_accent: role("ui.onAccent"),
            mode: role(mode.role()),
            unread: role("mail.unread"),
            flag: role("mail.flag"),
            attachment: role("mail.attachment"),
            quote: role("mail.quote"),
            link: role("mail.link"),
            header_key: role("mail.header.key"),
            error: role("state.error"),
            wallpaper: role(theme.ui.wallpaper_role.as_deref().unwrap_or("wallpaper")),
        }
    }
}

/// What every view needs: the grid and the colours.
#[derive(Clone, Copy, Debug)]
pub struct Ui {
    pub g: Grid,
    pub p: Paint,
    pub wallpaper_opacity: f32,
}

impl Ui {
    /// An element placed on the grid at `rect`.
    pub fn place(&self, e: El, rect: Rect) -> El {
        e.absolute(self.g.x(rect.x), self.g.y(rect.y)).size(self.g.x(rect.w), self.g.y(rect.h))
    }
}

/// Where the chrome sits, in points and cells, for a window of `size` points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chrome {
    pub cols: usize,
    pub rows: usize,
    /// The pane area, in cells from the window's top-left.
    pub panes: Rect,
    /// Left edge of the icon rail in points (it hugs the window's right edge), if it fits.
    pub rail_x: Option<f32>,
    /// Top of the workspace bar in points (it hugs the window's bottom edge).
    pub bar_y: f32,
}

impl Chrome {
    pub fn of(g: &Grid, size: [f32; 2]) -> Chrome {
        let (cols, rows) = (g.cols_in(size[0]), g.rows_in(size[1]));
        let rail = if cols >= 60 { RAIL_COLS } else { 0 };
        let panes = Rect::new(0, BAR_ROWS, cols.saturating_sub(rail), rows.saturating_sub(2 * BAR_ROWS));
        Chrome { cols, rows, panes, rail_x: (rail > 0).then(|| size[0] - g.x(rail)), bar_y: size[1] - g.y(BAR_ROWS) }
    }
}

pub fn root(s: &AppState, cx: &mut Cx<AppState>) -> El {
    let size = cx.layout_size(ROOT).unwrap_or(crate::WINDOW);
    let ui_spec = &s.theme.ui;
    let g = Grid::measure(ui_spec.font_size.unwrap_or(14.0), ui_spec.line_height.unwrap_or(1.35), cx.env().scale);
    let ui = Rc::new(Ui { g, p: Paint::of(&s.theme, &s.mode), wallpaper_opacity: ui_spec.wallpaper_opacity.unwrap_or(0.85) });
    let chrome = Chrome::of(&g, size);
    let narrow = chrome.panes.w < NARROW_COLS;
    s.frame.set(Frame { grid: g, panes: chrome.panes, narrow });

    let ws = s.ws();
    let layout = if narrow { ws.panes.layout_single(chrome.panes) } else { ws.panes.layout(chrome.panes) };
    let layout = if layout.pane(ws.panes.focus()).is_some() { layout } else { ws.panes.layout_single(chrome.panes) };
    let several = layout.panes.len() > 1;

    let mut root = el().key(ROOT).w_full().h_full().fill(ui.p.bg);
    root = root.child(tabs::bar(s, &ui, chrome.cols));
    for pane in &layout.panes {
        root = root.child(pane_view(s, &ui, pane, pane.id == ws.panes.focus(), several));
    }
    for (i, gutter) in layout.gutters.iter().enumerate() {
        root = root.child(gutter_view(&ui, i, gutter));
    }
    if let Some(x) = chrome.rail_x {
        root = root.child(rail::rail(&ui, chrome.panes.h).absolute(x, g.y(BAR_ROWS)));
    }
    root = root.child(workspaces::bar(s, &ui, chrome.cols).absolute(0.0, chrome.bar_y));
    if let Mode::Palette { query, selected } = &s.mode {
        root = root.child(palette::palette(&ui, query, *selected, chrome.cols, chrome.rows));
    }
    root
}

/// The cell-wide divider between two panes: a hairline down its middle, and a drag handle that
/// moves the divider in whole cells.
fn gutter_view(ui: &Ui, index: usize, gutter: &Gutter) -> El {
    let g = &ui.g;
    let axis = gutter.axis;
    let rule = match axis {
        Axis::Cols => el().absolute((g.cell_w / 2.0).floor(), 0.0).size(1.0, g.y(gutter.rect.h)),
        Axis::Rows => el().absolute(0.0, (g.cell_h / 2.0).floor()).size(g.x(gutter.rect.w), 1.0),
    };
    ui.place(el().key(format!("gutter:{index}")).role("separator").label("Pane divider"), gutter.rect)
        .cursor(match axis { Axis::Cols => Cursor::ResizeColumn, Axis::Rows => Cursor::ResizeRow })
        .child(rule.fill(ui.p.border).inert())
        .on_drag(move |s: &mut AppState, drag: &mut Drag, _tx| {
            let frame = s.frame.get();
            let [x, y] = drag.window_point();
            let cell = match axis { Axis::Cols => frame.grid.cols_in(x.max(0.0)), Axis::Rows => frame.grid.rows_in(y.max(0.0)) };
            s.ws_mut().panes.drag_gutter(index, cell, frame.panes);
        })
}

/// One pane: its content by kind, the status line when it has the focus, and the focus bar.
fn pane_view(s: &AppState, ui: &Rc<Ui>, pane: &Placed, focused: bool, several: bool) -> El {
    let g = &ui.g;
    let r = pane.rect;
    let status_rows = usize::from(focused && r.h > 3);
    let content = Rect::new(0, 0, r.w, r.h - status_rows);
    let body = match pane.kind {
        Kind::List => list::pane(s, ui, pane.id, content),
        Kind::Reader => reader::pane(s, ui, pane.id, content),
        Kind::Empty => wallpaper::pane(ui, content, "Empty pane", &wallpaper::hints()),
    };
    let role = match pane.kind { Kind::List => "list pane", Kind::Reader => "reader pane", Kind::Empty => "empty pane" };
    let mut e = ui.place(el().key(format!("pane:{}", pane.id)).role("region").label(role), r).child(body);
    if status_rows > 0 {
        e = e.child(statusline::line(s, ui, r.w).absolute(0.0, g.y(r.h - 1)));
    }
    if focused && several {
        e = e.child(el().key("focus-bar").absolute(0.0, 0.0).size(2.0, g.y(r.h)).fill(ui.p.focus_bar).inert());
    }
    let id = pane.id;
    e.on_press(move |s: &mut AppState| { s.ws_mut().panes.set_focus(id); })
}

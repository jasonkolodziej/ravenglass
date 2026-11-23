//! Ravenglass — M0 starting point (plan §6).
//!
//! What this does: opens a window in the Night theme and draws the braille raven wallpaper with
//! the empty-pane hint lines (plan §4 "Brand"). It is the first thing to get compiling.
//! Compiles against Rune at rev 0ca07561 on Rust 1.99.1; the snapshots below render.
//!
//! Run:       cargo run
//! Headless:  cargo run -- --snap out/wallpaper.png --size 1400x880 --scale 2
//!            cargo run -- --snap out/frost.png --scene frost
//!            cargo run -- --tree out/tree.json

use ravenglass_brand::{Dot, FieldSpec, cols_for, dot_field};
use ravenglass_theme::{Rgb, Theme};
use rune::tok::TextStyle;
use rune::{AppSpec, Color, Cx, El, col, el, text};

const WALLPAPER_ROWS: u32 = 26;

struct State {
    theme: Theme,
    dots: Vec<Dot>,
}

impl State {
    fn new() -> Self {
        Self { theme: Theme::night(), dots: dot_field(FieldSpec { rows: WALLPAPER_ROWS, facets: true }) }
    }
}

/// Theme colours are sRGB hex; Rune's `color::hex` returns its linear `Color`.
fn c(rgb: Rgb) -> Color {
    rune::color::hex(&rgb.hex()).unwrap_or(Color::WHITE)
}

/// The cell grid (plan §4 "The grid"). TODO(M2): measure the real advance of `0` in IBM Plex Mono
/// at the theme's size and round to device pixels; keep it in state.
struct Grid { cell_w: f32, cell_h: f32 }
const GRID: Grid = Grid { cell_w: 8.4, cell_h: 19.0 };

/// The braille raven: one small circle per dot on the 2×4-dots-per-cell grid.
/// TODO(M2): draw with `El::shape`/`canvas(Shape)` (one element, not thousands) and add the intro
/// (dots spring in from scattered positions on first open; skipped under Reduce Motion).
fn wallpaper(s: &State) -> El {
    let t = &s.theme;
    let (pitch_x, pitch_y) = (GRID.cell_w / 2.0, GRID.cell_h / 4.0);
    let d = pitch_x.min(pitch_y) * 0.62;
    let (w, h) = (cols_for(WALLPAPER_ROWS) as f32 * GRID.cell_w, WALLPAPER_ROWS as f32 * GRID.cell_h);
    let ink = c(t.role("wallpaper"));
    let eye = c(t.role("ui.accent"));
    let opacity = t.ui.wallpaper_opacity.unwrap_or(0.85);
    let mut field = el().key("wallpaper").size(w, h).inert().opacity(opacity);
    for dot in &s.dots {
        let (x, y) = (dot.x as f32 * pitch_x + (pitch_x - d) / 2.0, dot.y as f32 * pitch_y + (pitch_y - d) / 2.0);
        field = field.child(el().size(d, d).radius(d / 2.0).fill(if dot.eye { eye } else { ink }).absolute(x, y));
    }
    field
}

fn view(s: &State, _cx: &mut Cx<State>) -> El {
    let t = &s.theme;
    let mono = TextStyle::new(14.0, 400.0).mono();
    col().key("pane").w_full().h_full().center().gap(14.0).fill(c(t.role("ui.bg")))
        .child(wallpaper(s))
        .child(text("R A V E N G L A S S").font(TextStyle::new(14.0, 600.0).mono()).color(c(t.role("ui.text"))))
        .child(text("No message open").font(mono).color(c(t.role("ui.muted"))))
        .child(text("/ search    ⇧⌘P commands    1–9 workspaces    c compose").font(mono).color(c(t.role("ui.muted"))))
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[
            ("night", |s| s.theme = Theme::night()),
            ("frost", |s| s.theme = Theme::frost()),
        ],
        ..AppSpec::new("Ravenglass", (1400.0, 880.0), State::new, view)
    });
}

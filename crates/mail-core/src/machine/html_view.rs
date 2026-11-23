//! `HtmlView`: the HTML rendering of one open message (architecture §7). Layout is reused when
//! only paint inputs change; any failure ends in the text view.

use serde::{Deserialize, Serialize};

/// Tiles are this many device pixels tall.
pub const TILE_H: u32 = 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum State {
    Parsing { width: u32 },
    LaidOut { width: u32, height: u32 },
    Rastering { width: u32, height: u32, pending: u32 },
    Ready { width: u32, height: u32 },
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Parsed,
    LaidOut { height: u32 },
    TileDone,
    /// The pane settled at a new width (the caller debounces a drag).
    Width(u32),
    /// Scale, theme, image policy or find highlights changed: same layout width, new pixels.
    Repaint,
    /// A panic, a timeout or a size overflow in the renderer.
    Crashed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect { Layout { width: u32 }, Raster { tiles: u32 }, Fallback }

pub fn open(width: u32) -> State { State::Parsing { width } }

pub fn tiles_for(height: u32) -> u32 { height.div_ceil(TILE_H).max(1) }

pub fn step(s: &mut State, ev: Event) -> Vec<Effect> {
    use State::*;
    match (s.clone(), ev) {
        (Failed, _) => vec![],
        (_, Event::Crashed) => { *s = Failed; vec![Effect::Fallback] }
        (Parsing { width }, Event::Parsed) => vec![Effect::Layout { width }],
        (Parsing { width }, Event::LaidOut { height }) => { *s = LaidOut { width, height }; start_raster(s) }
        (Parsing { .. }, Event::Width(width)) => { *s = Parsing { width }; vec![] }
        (Rastering { width, height, pending }, Event::TileDone) => {
            *s = if pending <= 1 { Ready { width, height } } else { Rastering { width, height, pending: pending - 1 } };
            vec![]
        }
        (LaidOut { width, .. } | Rastering { width, .. } | Ready { width, .. }, Event::Width(w)) if w != width => {
            *s = Parsing { width: w };
            vec![Effect::Layout { width: w }]
        }
        (LaidOut { width, height } | Rastering { width, height, .. } | Ready { width, height }, Event::Repaint) => {
            *s = LaidOut { width, height };
            start_raster(s)
        }
        _ => vec![],
    }
}

fn start_raster(s: &mut State) -> Vec<Effect> {
    let State::LaidOut { width, height } = *s else { return vec![] };
    let tiles = tiles_for(height);
    *s = State::Rastering { width, height, pending: tiles };
    vec![Effect::Raster { tiles }]
}

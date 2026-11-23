//! Ravenglass: a character-grid mail client on Rune (plan §4).
//!
//! Run:       cargo run -p ravenglass                      (your profile; `:account add you@example.com`)
//!            cargo run -p ravenglass -- --demo            (the built-in demo accounts)
//!            cargo run -p ravenglass -- --profile <dir>   (or RAVENGLASS_PROFILE: everything in one folder)
//! Headless:  cargo run -p ravenglass -- --snap out/default.png --scene default --size 1400x880 --scale 2
//!            cargo run -p ravenglass -- --snap out/narrow.png --scene narrow --size 800x700 --scale 2
//!            cargo run -p ravenglass -- --tree out/tree.json --scene default
//!            cargo run -p ravenglass -- --script app/scripts/triage.json --out out/triage/
//!            cargo run -p ravenglass -- --list-scenes

mod accounts_ui;
mod actions;
mod compose;
mod demo;
mod fold;
mod grid;
mod jobs;
mod keymap;
mod keys;
mod link;
mod links;
mod lock;
mod modes;
mod panes;
mod platform;
mod profile;
mod scenes;
mod settings;
mod shell;
mod state;
mod timefmt;
mod update;
mod view;

#[cfg(test)]
mod tests;

use keys::KeyPress;
use rune::{AppSpec, KeyEvent, Tx, Waker};
use state::{AppState, Boot, Fx};
use std::time::Duration;

/// The window's size before the first layout, in points.
pub const WINDOW: [f32; 2] = [1400.0, 880.0];

/// IBM Plex Mono, the UI font: everything is drawn in it, so it fills both of Rune's font slots.
pub const FONTS: &[&[u8]] = &[
    include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexMono-Medium.ttf"),
];
pub const FAMILIES: (&str, &str) = ("IBM Plex Mono", "IBM Plex Mono");

/// The demo accounts (`--demo`, and headless runs that name no profile) or the profile on disk
/// (`--profile <dir>`, `RAVENGLASS_PROFILE`, else the platform's folders): see `shell.rs`.
fn init() -> AppState { AppState::launch() }

fn setup(s: &mut AppState, waker: Waker) { s.waker = Some(waker); }

/// Carry out what state changes asked of the window.
pub fn flush(s: &mut AppState, tx: &mut Tx) {
    for fx in std::mem::take(&mut s.fx) {
        match fx {
            Fx::Reveal(key) => tx.scroll_into_view(key),
            Fx::ScrollRows(key, rows) => tx.scroll_to(key, 0.0, s.frame.get().grid.y(rows)),
            Fx::ScrollTop(key) => tx.scroll_to_y(key, 0.0),
            Fx::WakeAt(due) => tx.wake_at(due),
            Fx::ThemeChanged => tx.color_motion(rune::tok::motion::CALM),
            Fx::Quit => tx.quit(),
            Fx::Blur => tx.blur(),
            Fx::Instant => tx.instant(),
            Fx::OpenUrl(url) => if let Err(reason) = platform::open_url(tx, &url) { s.error(reason); },
        }
    }
}

fn poll(s: &mut AppState, clock: f64, tx: &mut Tx) -> bool {
    let mut changed = s.tick(clock);
    changed |= s.shell_poll(clock);
    match s.boot {
        // The engine starts once the first frame is out.
        Boot::Cold => {
            s.boot = Boot::Armed;
            tx.wake_after(Duration::from_millis(1));
        }
        Boot::Armed => {
            s.start_engine();
            changed = true;
        }
        Boot::Started => {}
    }
    if s.pump() {
        // Mail that lands just after launch should not animate in.
        if clock < 1.0 { tx.instant(); }
        changed = true;
    }
    flush(s, tx);
    changed
}

fn on_key(s: &mut AppState, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.clock = tx.clock();
    let Some(press) = KeyPress::from_event(ev) else { return false };
    let acted = s.key(press);
    flush(s, tx);
    acted
}

pub fn spec() -> AppSpec<AppState> {
    AppSpec::<AppState> {
        scenes: scenes::SCENES,
        fonts: FONTS,
        families: FAMILIES,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        min_size: Some((480.0, 320.0)),
        ..AppSpec::new("Ravenglass", (WINDOW[0], WINDOW[1]), init, view::root)
    }
}

fn main() {
    rune::run(spec());
}

//! Links the system opens with the app: a `mailto:` link clicked anywhere becomes a new draft.
//!
//! - macOS hands the link to the running app as an Apple event (`App::on_open_url`).
//! - Windows and Linux start a new process with the link as an argument. Rune forwards a later
//!   launch's links to the running process, but only links written `scheme://…`, which
//!   `mailto:` is not. So [`relay_launch_link`] restarts such a launch with the same link wrapped
//!   as `ravenglass://mailto?u=…`; Rune then delivers it to the running app, or, when none runs,
//!   to this new one as its launch link.
//!
//! A link comes from outside the app: it only fills in a draft for the person to read, and only
//! with recipients, a subject and a body (`mail_render::mailto`).

// The web build gets links through the page, not the OS; its entry is `web::spec`.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use crate::state::AppState;
use mail_render::mailto::{self, Mailto};
use rune::Tx;

/// The app's identity for `rune::single_instance`.
pub const APP_ID: &str = "io.github.jasonkolodziej.Ravenglass";

/// `App::on_open_url`: the system opened a link with the app.
pub fn on_open_url(s: &mut AppState, url: &str, tx: &mut Tx) {
    s.clock = tx.clock();
    s.open_system_link(url);
    crate::flush(s, tx);
}

impl AppState {
    /// Act on a link the system opened with the app. Anything but a `mailto:` link is ignored.
    pub fn open_system_link(&mut self, url: &str) {
        let Some(link) = mailto::unwrap_link(url).and_then(|inner| Mailto::parse(&inner)) else { return };
        if self.link.is_off() {
            // The engine starts after the first frame: the draft waits for it.
            self.compose.waiting_link = Some(link);
            return;
        }
        self.compose_from_link(link);
    }

    fn compose_from_link(&mut self, link: Mailto) {
        self.compose.prefill = Some(link);
        self.compose_new();
        // No account to write from: the link has nowhere to go.
        if !self.compose.is_opening() { self.compose.prefill = None; }
    }

    /// The engine just started: a link that arrived before it opens its draft now.
    pub fn open_waiting_link(&mut self) {
        if let Some(link) = self.compose.waiting_link.take() { self.compose_from_link(link); }
    }
}

/// Windows and Linux: when this launch carries a `mailto:` argument, start the app again with
/// the link wrapped so Rune can hand it on, and say so (the caller then exits).
pub fn relay_launch_link(args: &[String]) -> bool {
    if cfg!(target_os = "macos") { return false; }
    let Some(link) = args.iter().find(|a| a.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("mailto:"))) else { return false };
    let Ok(exe) = std::env::current_exe() else { return false };
    let wrapped = format!("ravenglass://mailto?u={}", mailto::encode(link));
    std::process::Command::new(exe).arg(wrapped).spawn().is_ok()
}

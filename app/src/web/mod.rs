//! The web build (architecture §8): the same app in a page, signed in to a Relay.
//!
//! - [`session`]: the Relay sign-in as a state machine (pure; tested on every target).
//! - [`screen`]: the sign-in screen, in the grid.
//! - `host`: the browser glue: the app's `poll`, `on_key` and view for a page, the engine as a
//!   task on the page thread, settings in `localStorage`. Compiled for wasm only.
//!
//! On desktop only the tests use the first two.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

pub mod screen;
pub mod session;

#[cfg(target_arch = "wasm32")]
mod host;
#[cfg(target_arch = "wasm32")]
pub use host::{Web, spec};

#[cfg(test)]
mod tests;

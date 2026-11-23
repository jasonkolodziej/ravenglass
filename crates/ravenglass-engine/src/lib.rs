//! The Ravenglass engine (architecture §3, §5): everything below the message boundary.
//!
//! The UI sends [`Command`]s and renders [`Event`]s; the engine owns the backends, the store, the
//! sync and outbox state machines and the offline journal. It imports no UI types, so it runs on
//! a desktop thread ([`runner`]), in a Web Worker and in tests. [`Engine::handle`] is a plain
//! `async fn` with no executor of its own.
//!
//! HTML mail is laid out and painted by `mail-blitz` on its render thread; the engine holds the
//! [`Renderer`] handle and one `HtmlView` state machine per reader pane.
//!
//! Not here yet: draft saving (`ComposeSession`), fetching remote images the user allowed (until
//! then they stay placeholders), server search for mail outside the cache, and the IDLE
//! subscription that turns pushes into [`Command::Refresh`].

pub mod api;
mod engine;
mod html;
mod ops;
#[cfg(not(target_arch = "wasm32"))]
pub mod runner;
#[cfg(test)]
mod tests;

pub use api::{BodyView, Command, Event, OpId, OutboxId, ReaderView, ViewId};
pub use mail_blitz::{DarkMode, Failure as HtmlFailure, Info as HtmlInfo, Link, Options as HtmlOptions, Renderer, Tile};
pub use engine::{Config, Engine, PAGE};

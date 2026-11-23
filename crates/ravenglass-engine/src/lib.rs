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
//! Not here yet: rich-text drafts, deleting a sent draft's server copy, and server search for
//! mail outside the cache. Allowed remote images are fetched on the engine thread, one after
//! another, so a slow image host delays other commands for up to its timeout.

pub mod api;
mod drafts;
mod engine;
#[cfg(feature = "http")]
pub mod fetch;
mod html;
mod ops;
#[cfg(not(target_arch = "wasm32"))]
pub mod runner;
#[cfg(test)]
mod tests;

pub use api::{BodyView, Command, DraftSource, Event, OpId, OutboxId, ReaderView, ViewId};
pub use mail_render::compose::ReplyKind;
pub use mail_blitz::{DarkMode, Failure as HtmlFailure, Info as HtmlInfo, Link, Options as HtmlOptions, Renderer, Tile};
pub use engine::{Config, Engine, ImageFetcher, PAGE, POLL_SECS};

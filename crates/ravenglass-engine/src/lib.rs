//! The Ravenglass engine (architecture §3, §5): everything below the message boundary.
//!
//! The UI sends [`Command`]s and renders [`Event`]s; the engine owns the backends, the store, the
//! sync and outbox state machines and the offline journal. It imports no UI types, so it runs on
//! a desktop thread ([`runner`]), in a Web Worker and in tests. [`Engine::handle`] is a plain
//! `async fn` with no executor of its own.
//!
//! Not here yet: draft saving (`ComposeSession`), HTML layout and tiles (`mail-blitz`), server
//! search for mail outside the cache, and the IDLE subscription that turns pushes into
//! [`Command::Refresh`].

pub mod api;
mod engine;
mod ops;
#[cfg(not(target_arch = "wasm32"))]
pub mod runner;
#[cfg(test)]
mod tests;

pub use api::{BodyView, Command, Event, OpId, OutboxId, ReaderView};
pub use engine::{Config, Engine, PAGE};

//! Ravenglass domain model. UI-free and I/O-free: backends implement [`MailBackend`], the app drives
//! the state machines in [`machine`] and performs their effects.

pub mod backend;
mod exec;
pub mod machine;
pub mod types;

pub use backend::{BoxFut, MailBackend};
#[cfg(not(target_arch = "wasm32"))]
pub use exec::block_on;
pub use types::*;

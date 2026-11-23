//! Ravenglass domain model. UI-free and I/O-free: backends implement [`MailBackend`], the app drives
//! the state machines in [`machine`] and performs their effects.

pub mod backend;
pub mod machine;
pub mod types;

pub use backend::{BoxFut, MailBackend};
pub use types::*;

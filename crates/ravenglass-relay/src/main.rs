//! `ravenglass-relay`: run the Relay. Configuration comes from the environment so no secret is
//! ever a command-line argument:
//!
//! - `RAVENGLASS_RELAY_TOKEN` (required): the shared bearer token, at least 16 characters.
//! - `RAVENGLASS_RELAY_LISTEN` (default `127.0.0.1:8743`): put a TLS proxy in front of it.
//! - `RAVENGLASS_RELAY_ORIGIN`: the web app's origin, for browser access.
//! - `RAVENGLASS_RELAY_DEMO=1`: serve the built-in demo accounts. Real accounts are not wired
//!   in yet, so without this the Relay starts with none.

use ravenglass_relay::{Relay, server};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let token = var("RAVENGLASS_RELAY_TOKEN").ok_or("set RAVENGLASS_RELAY_TOKEN")?;
    let listen = var("RAVENGLASS_RELAY_LISTEN").unwrap_or_else(|| "127.0.0.1:8743".into());
    let mut relay = Relay::new();
    if var("RAVENGLASS_RELAY_DEMO").is_some() {
        for backend in mail_mock::MockBackend::demo() { relay.add_backend(Arc::new(backend)); }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(&listen).await?;
        eprintln!("ravenglass-relay listening on {}", listener.local_addr()?);
        server::serve(listener, relay, server::Config { token, origin: var("RAVENGLASS_RELAY_ORIGIN") }).await
    })?;
    Ok(())
}

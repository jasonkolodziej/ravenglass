//! The browser, as the web build's platform: the page's settings, web storage, and the status
//! line mirrored into the page for assistive technology.
//!
//! What may be kept where (architecture §6, §12): settings and the theme in `localStorage`; a
//! single-user Relay's token in `sessionStorage` only (it goes when the tab closes); nothing
//! else secret, ever. Every accessor fails soft: a browser with storage turned off still runs,
//! it just forgets.

use web_sys::{Document, Storage};

/// `<meta name="ravenglass-relay" content="https://relay.example.org">` in `index.html` names
/// the Relay; without it the Relay is this page's own origin.
const RELAY_META: &str = "meta[name=ravenglass-relay]";
/// The element that mirrors the status line, from `index.html` (`role="status"`).
const STATUS_ID: &str = "rg-status";

fn document() -> Option<Document> { web_sys::window()?.document() }

/// The Relay's origin, without a trailing slash; empty for this page's own origin.
pub fn relay_origin() -> String {
    let content = document().and_then(|d| d.query_selector(RELAY_META).ok().flatten()).and_then(|m| m.get_attribute("content"));
    content.map(|c| c.trim().trim_end_matches('/').to_owned()).unwrap_or_default()
}

/// The Relay as the sign-in screen names it.
pub fn relay_label(origin: &str) -> String {
    if !origin.is_empty() { return origin.to_owned(); }
    web_sys::window().and_then(|w| w.location().origin().ok()).unwrap_or_else(|| "this site".into())
}

/// Which storage a value goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// `localStorage`: settings, kept across visits.
    Settings,
    /// `sessionStorage`: gone when the tab closes. The only place a token may be kept.
    Tab,
}

fn storage(keep: Keep) -> Option<Storage> {
    let window = web_sys::window()?;
    match keep { Keep::Settings => window.local_storage(), Keep::Tab => window.session_storage() }.ok().flatten()
}

pub fn read(keep: Keep, key: &str) -> Option<String> { storage(keep)?.get_item(key).ok().flatten() }

pub fn write(keep: Keep, key: &str, value: &str) { if let Some(s) = storage(keep) { let _ = s.set_item(key, value); } }

pub fn forget(keep: Keep, key: &str) { if let Some(s) = storage(keep) { let _ = s.remove_item(key); } }

/// Mirror the status line into the page's live region, so a screen reader hears the mode and
/// the sync state change (the canvas itself has no accessible text yet).
pub fn announce(text: &str) {
    let Some(el) = document().and_then(|d| d.get_element_by_id(STATUS_ID)) else { return };
    if el.text_content().as_deref() != Some(text) { el.set_text_content(Some(text)); }
}

/// Name a fact about the page on the root element, `data-rg-<name>`, for scripts and tests that
/// drive the page from outside (the canvas has no DOM to query).
pub fn mark(name: &str, value: &str) {
    let Some(root) = document().and_then(|d| d.document_element()) else { return };
    let attr = format!("data-rg-{name}");
    if root.get_attribute(&attr).as_deref() != Some(value) { let _ = root.set_attribute(&attr, value); }
}

/// Ask the browser not to evict this origin's storage under pressure (architecture §6).
pub fn persist_storage() {
    let Some(manager) = web_sys::window().map(|w| w.navigator().storage()) else { return };
    if let Ok(promise) = manager.persist() {
        wasm_bindgen_futures::spawn_local(async move { let _ = wasm_bindgen_futures::JsFuture::from(promise).await; });
    }
}

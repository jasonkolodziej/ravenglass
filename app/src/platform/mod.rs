//! What the app asks of the operating system. Only [`open_url`] so far; per-OS code goes in
//! `platform/{macos,windows,linux}.rs` with a portable default here.

use std::sync::Mutex;

/// URLs handed to [`open_url`] in headless runs and tests, which never start a browser.
static RECORDED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The schemes a link in a message may open.
const SCHEMES: [&str; 3] = ["https://", "http://", "mailto:"];

/// Open `url` with the system's handler (the default browser for web links), off the calling
/// thread. Headless runs and tests record the URL instead; see [`recorded`].
pub fn open_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if !SCHEMES.iter().any(|s| lower.starts_with(s)) { return Err("only web and mail links can be opened".into()); }
    if cfg!(test) || rune::tooling::is_headless() {
        if let Ok(mut recorded) = RECORDED.lock() { recorded.push(url.to_owned()); }
        return Ok(());
    }
    open::that_detached(url).map_err(|e| e.to_string())
}

/// What [`open_url`] recorded instead of opening, oldest first.
#[cfg(test)]
pub fn recorded() -> Vec<String> { RECORDED.lock().map(|r| r.clone()).unwrap_or_default() }

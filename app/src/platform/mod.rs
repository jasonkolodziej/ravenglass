//! The platform layer: the only place code differs by operating system (CLAUDE.md). Each feature
//! has a portable default here; `linux.rs`, `macos.rs` and `windows.rs` replace it where the OS
//! has something better. The per-OS path functions are pure (they take the environment as an
//! argument), so every platform's layout is compiled and tested on every platform.

pub mod linux;
pub mod macos;
pub mod windows;

use mail_auth::{SecretKind, SecretStore};
use mail_core::AccountId;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// An environment lookup (`std::env::var` in the app, a map in tests).
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The folder name on platforms that capitalize application folders.
pub const APP_FOLDER: &str = "Ravenglass";
/// The folder name under the XDG base directories.
pub const XDG_FOLDER: &str = "ravenglass";

/// Where the profile lives (architecture §6): settings in `config`, the mail cache in `data`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    pub config: PathBuf,
    pub data: PathBuf,
}

/// An absolute path from the environment; relative values are ignored, as the XDG spec says.
pub(crate) fn absolute(env: Env, key: &str) -> Option<PathBuf> {
    env(key).filter(|v| !v.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute() || looks_like_windows_path(p))
}

/// `C:\…` is absolute on Windows, and has to stay so when the layout is tested elsewhere.
fn looks_like_windows_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let mut chars = s.chars();
    matches!((chars.next(), chars.next(), chars.next()), (Some(d), Some(':'), Some('\\' | '/')) if d.is_ascii_alphabetic())
}

/// The portable default: the XDG base directories under the home folder.
pub fn xdg_dirs(env: Env) -> Option<Dirs> {
    let home = absolute(env, "HOME");
    let base = |key: &str, fallback: &str| absolute(env, key).or_else(|| home.as_ref().map(|h| h.join(fallback)));
    Some(Dirs { config: base("XDG_CONFIG_HOME", ".config")?.join(XDG_FOLDER), data: base("XDG_DATA_HOME", ".local/share")?.join(XDG_FOLDER) })
}

/// This platform's profile folders. `None` when the environment names no home folder.
pub fn dirs(env: Env) -> Option<Dirs> {
    #[cfg(target_os = "macos")]
    { macos::dirs(env) }
    #[cfg(target_os = "windows")]
    { windows::dirs(env) }
    #[cfg(target_os = "linux")]
    { linux::dirs(env) }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    { xdg_dirs(env) }
}

/// Keep a profile file or folder to its owner where the OS has per-file modes. The portable
/// default changes nothing: the file inherits the profile folder's protection.
pub fn restrict(path: &Path, is_dir: bool) {
    #[cfg(target_os = "macos")]
    macos::restrict(path, is_dir);
    #[cfg(target_os = "linux")]
    linux::restrict(path, is_dir);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = (path, is_dir);
}

/// How the app lock is opened on this platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unlock {
    /// The OS prompt (Touch ID, Windows Hello) answered.
    #[allow(dead_code)] // no platform has its OS prompt wired up yet (see macos.rs, windows.rs)
    Os(bool),
    /// No OS prompt here: the lock screen asks for an account password and
    /// [`check_password`] compares it.
    Password,
}

/// Ask the OS to authenticate the user. The portable default has no OS prompt.
pub fn authenticate() -> Unlock {
    #[cfg(target_os = "macos")]
    { macos::authenticate() }
    #[cfg(target_os = "windows")]
    { windows::authenticate() }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { Unlock::Password }
}

/// The portable unlock: `typed` against the stored password of `account`, compared in constant
/// time. False when nothing is stored or the store cannot be read.
pub fn check_password(secrets: &dyn SecretStore, account: &AccountId, typed: &str) -> bool {
    match secrets.get(account, SecretKind::Password) {
        Ok(Some(stored)) => constant_time_eq(stored.as_bytes(), typed.as_bytes()),
        _ => false,
    }
}

/// Equality that takes the same time wherever the first difference is. The length of the longer
/// input still shows, which a password check cannot avoid without hashing both.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
}

/// Open a link in the system's default handler. Rune does the per-OS work (`NSWorkspace`,
/// `ShellExecuteW`, `xdg-open`) and refuses anything but `http`, `https` and `mailto`.
pub fn open_url(tx: &mut rune::Tx, url: &str) -> Result<(), String> {
    tx.open_url(url).map_err(|e| e.to_string())
}


/// URLs handed to [`open_link`] in headless runs and tests, which never start a browser.
static RECORDED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The schemes a link in a message may open.
const SCHEMES: [&str; 3] = ["https://", "http://", "mailto:"];

/// Open a link the user clicked in a message with the system's handler (the default browser for web links), off the calling
/// thread. Headless runs and tests record the URL instead; see [`recorded`].
pub fn open_link(url: &str) -> Result<(), String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if !SCHEMES.iter().any(|s| lower.starts_with(s)) { return Err("only web and mail links can be opened".into()); }
    if cfg!(test) || rune::tooling::is_headless() {
        if let Ok(mut recorded) = RECORDED.lock() { recorded.push(url.to_owned()); }
        return Ok(());
    }
    open::that_detached(url).map_err(|e| e.to_string())
}

/// What [`open_link`] recorded instead of opening, oldest first.
#[cfg(test)]
pub fn recorded() -> Vec<String> { RECORDED.lock().map(|r| r.clone()).unwrap_or_default() }

//! Windows: `%APPDATA%\Ravenglass` for settings and data alike, and (not yet) Windows Hello for
//! the app lock. Files under `%APPDATA%` are already private to the user, so `restrict` keeps the
//! portable default.

// Every platform's layout is compiled and tested on every platform, so on any one of them the
// other two are unused outside tests.
#![allow(dead_code)]

#[cfg(target_os = "windows")]
use super::Unlock;
use super::{APP_FOLDER, Dirs, Env};

#[cfg_attr(not(target_os = "windows"), allow(dead_code))] // tested everywhere, used on Windows
pub fn dirs(env: Env) -> Option<Dirs> {
    let roaming = super::absolute(env, "APPDATA").or_else(|| super::absolute(env, "USERPROFILE").map(|home| home.join("AppData").join("Roaming")))?;
    let root = roaming.join(APP_FOLDER);
    Some(Dirs { config: root.clone(), data: root })
}

/// TODO(app lock, v1): Windows Hello through `Windows.Security.Credentials.UI.UserConsentVerifier`
/// (`RequestVerificationForWindowAsync`). It needs the WinRT bindings and a Windows machine to
/// verify on; until then the lock screen asks for an account password, the portable default.
#[cfg(target_os = "windows")]
pub fn authenticate() -> Unlock { Unlock::Password }

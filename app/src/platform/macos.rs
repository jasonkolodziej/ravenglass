//! macOS: `~/Library/Application Support/Ravenglass` for settings and data alike, owner-only
//! modes on the profile, and (not yet) Touch ID for the app lock.

#[cfg(target_os = "macos")]
use super::Unlock;
use super::{APP_FOLDER, Dirs, Env};
#[cfg(target_os = "macos")]
use std::path::Path;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // tested everywhere, used on macOS
pub fn dirs(env: Env) -> Option<Dirs> {
    let root = super::absolute(env, "HOME")?.join("Library").join("Application Support").join(APP_FOLDER);
    Some(Dirs { config: root.clone(), data: root })
}

#[cfg(target_os = "macos")]
pub fn restrict(path: &Path, is_dir: bool) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(if is_dir { 0o700 } else { 0o600 }));
}

/// TODO(app lock, v1): Touch ID through LocalAuthentication (`LAContext.evaluatePolicy` with
/// `deviceOwnerAuthentication`, which also offers the login password). It needs Objective-C
/// bindings and a Mac to verify on; until then the lock screen asks for an account password,
/// the portable default.
#[cfg(target_os = "macos")]
pub fn authenticate() -> Unlock { Unlock::Password }

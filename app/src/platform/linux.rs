//! Linux: XDG base directories, and owner-only modes on the profile.

use super::{Dirs, Env};
#[cfg(target_os = "linux")]
use std::path::Path;

/// `$XDG_CONFIG_HOME/ravenglass` and `$XDG_DATA_HOME/ravenglass`, defaulting to `~/.config` and
/// `~/.local/share`. This is the portable default.
pub fn dirs(env: Env) -> Option<Dirs> { super::xdg_dirs(env) }

/// 0700 for folders and 0600 for files: accounts.toml names servers and logins, mail.db holds mail.
#[cfg(target_os = "linux")]
pub fn restrict(path: &Path, is_dir: bool) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(if is_dir { 0o700 } else { 0o600 }));
}

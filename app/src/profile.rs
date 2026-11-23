//! The profile on disk (architecture §6, appendix A): `config.toml`, `accounts.toml` and
//! `themes/*.toml` in the config folder, `mail.db` in the data folder. `--profile <dir>` or
//! `RAVENGLASS_PROFILE` puts all of it in one folder, which is what tests and scripts use.
//!
//! Nothing here holds a secret: passwords and tokens live in the `SecretStore` only. Reading
//! happens once before the first frame; every write runs on a job thread (`shell.rs`).

use crate::platform::{self, Env};
use mail_auth::AccountConfig;
use ravenglass_config::Config;
use ravenglass_theme::{Appearance, Theme, ThemeFile};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "config.toml";
pub const ACCOUNTS_FILE: &str = "accounts.toml";
pub const THEMES_DIR: &str = "themes";
pub const STORE_FILE: &str = "mail.db";
/// The environment variable that names the profile folder.
pub const PROFILE_ENV: &str = "RAVENGLASS_PROFILE";
/// Settings and theme files larger than this are refused (architecture §12).
const MAX_FILE: u64 = 256 << 10;

/// What the command line asked of the app itself (Rune takes its own flags first).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Run on the built-in demo accounts and touch no profile.
    pub demo: bool,
    pub profile: Option<String>,
}

pub fn parse_args(args: &[String]) -> Options {
    let mut out = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => out.demo = true,
            "--profile" => out.profile = args.next().cloned(),
            other => if let Some(dir) = other.strip_prefix("--profile=") { out.profile = Some(dir.to_owned()); },
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

/// Everything read from the profile at startup. A file that does not parse is reported in
/// `problems` (one line each) and its defaults are used; it is never overwritten because of it.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub config: Config,
    pub accounts: Vec<AccountConfig>,
    pub themes: Vec<Theme>,
    pub problems: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountsFile {
    #[serde(default, rename = "account")]
    accounts: Vec<AccountConfig>,
}

/// The keys of one `[[account]]`: the fields of `mail_auth::AccountConfig`.
const ACCOUNT_KEYS: [&str; 8] = ["id", "name", "address", "username", "incoming", "smtp", "auth", "server_saves_sent"];

/// `accounts.toml` for these accounts. It never contains a secret: `AccountConfig` has none.
pub fn accounts_to_toml(accounts: &[AccountConfig]) -> Result<String, String> {
    toml::to_string(&AccountsFile { accounts: accounts.to_vec() }).map_err(|e| format!("accounts.toml: {e}"))
}

/// Parse `accounts.toml`. Errors name the problem without quoting the file, which holds addresses.
pub fn accounts_from_toml(src: &str) -> Result<Vec<AccountConfig>, String> {
    // `AccountConfig` ignores keys it does not know, and a `password = "…"` line that silently
    // does nothing would sit in the file for good: refuse unknown keys, naming only the key.
    let table: toml::Table = toml::from_str(src).map_err(|e| format!("accounts.toml: {}", e.message()))?;
    let entries = table.get("account").and_then(|a| a.as_array()).into_iter().flatten().filter_map(|a| a.as_table());
    for key in entries.flat_map(|entry| entry.keys()) {
        if !ACCOUNT_KEYS.contains(&key.as_str()) { return Err(format!("accounts.toml: unknown key `{}` (secrets are never kept in this file)", key.chars().take(32).collect::<String>())); }
    }
    let file: AccountsFile = toml::from_str(src).map_err(|e| format!("accounts.toml: {}", e.message()))?;
    let mut seen: Vec<&str> = Vec::new();
    for account in &file.accounts {
        let id = account.id.0.as_str();
        if !valid_account_id(id) { return Err("accounts.toml: an account id must be letters, digits, `-`, `_` or `.`".into()); }
        if seen.contains(&id) { return Err(format!("accounts.toml: account {id} is defined twice")); }
        seen.push(id);
    }
    Ok(file.accounts)
}

/// Ids name keyring entries and prefix folder ids, so they stay plain.
pub fn valid_account_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A file-name-safe id from a theme or account name: lower-case letters, digits and dashes.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() { out.push(c.to_ascii_lowercase()); } else if !out.ends_with('-') && !out.is_empty() { out.push('-'); }
    }
    out.trim_end_matches('-').chars().take(48).collect()
}

/// A user theme from its file text. `stem` (the file name without `.toml`) is its id and name
/// when the file sets none. Built-in ids cannot be replaced.
pub fn parse_user_theme(src: &str, stem: &str, known: &[Theme]) -> Result<Theme, String> {
    let file: ThemeFile = toml::from_str(src).map_err(|e| e.message().to_owned())?;
    let mut theme = Theme::parse(src, known).map_err(|e| e.to_string())?;
    if file.id.is_none() { theme.id = slug(stem); }
    if file.name.is_none() { theme.name = stem.to_owned(); }
    if theme.id.is_empty() { return Err("the theme has no id".into()); }
    if Theme::builtin(&theme.id).is_some() || matches!(theme.id.as_str(), "system" | "night" | "frost") { return Err(format!("`{}` is a built-in theme id", theme.id)); }
    // A terminal scheme layered on a base keeps the base's appearance; its background decides.
    if file.appearance.is_none() && file.palette.contains_key("background") {
        theme.appearance = if theme.color("background").luminance() > 0.35 { Appearance::Light } else { Appearance::Dark };
    }
    Ok(theme)
}

/// A whole theme as a file: every palette entry and role, so it stands without a base.
pub fn theme_to_toml(theme: &Theme) -> String {
    use std::fmt::Write;
    let quoted = |s: &str| s.chars().filter(|c| !c.is_control()).collect::<String>().replace('\\', "\\\\").replace('"', "\\\"");
    let mut out = String::new();
    let appearance = match theme.appearance { Appearance::Dark => "dark", Appearance::Light => "light" };
    let _ = writeln!(out, "name = \"{}\"\nid = \"{}\"\nappearance = \"{appearance}\"\nversion = 1\n\n[palette]", quoted(&theme.name), quoted(&theme.id));
    for name in ravenglass_theme::NAMES { let _ = writeln!(out, "{name} = \"{}\"", theme.color(name).hex()); }
    out.push_str("\n[roles]\n");
    for (role, name) in &theme.roles { let _ = writeln!(out, "\"{}\" = \"{}\"", quoted(role), quoted(name)); }
    out.push_str("\n[ui]\n");
    let ui = &theme.ui;
    if let Some(v) = &ui.font { let _ = writeln!(out, "font = \"{}\"", quoted(v)); }
    if let Some(v) = ui.font_size { let _ = writeln!(out, "font_size = {v}"); }
    if let Some(v) = ui.line_height { let _ = writeln!(out, "line_height = {v}"); }
    if let Some(v) = ui.window_opacity { let _ = writeln!(out, "window_opacity = {v}"); }
    if let Some(v) = ui.window_blur { let _ = writeln!(out, "window_blur = {v}"); }
    if let Some(v) = &ui.wallpaper { let _ = writeln!(out, "wallpaper = \"{}\"", quoted(v)); }
    if let Some(v) = &ui.wallpaper_role { let _ = writeln!(out, "wallpaper_role = \"{}\"", quoted(v)); }
    if let Some(v) = ui.wallpaper_opacity { let _ = writeln!(out, "wallpaper_opacity = {v}"); }
    if let Some(v) = ui.intro { let _ = writeln!(out, "intro = {v}"); }
    out
}

/// Read a settings file. `Ok(None)`: it does not exist.
pub fn read_capped(path: &Path) -> Result<Option<String>, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{name}: {}", e.kind())),
    };
    if meta.len() > MAX_FILE { return Err(format!("{name}: larger than {} KB", MAX_FILE >> 10)); }
    std::fs::read_to_string(path).map(Some).map_err(|e| format!("{name}: {}", e.kind()))
}

impl Profile {
    /// Everything in one folder (`--profile`, `RAVENGLASS_PROFILE`).
    pub fn at(dir: impl Into<PathBuf>) -> Profile {
        let dir = dir.into();
        Profile { config_dir: dir.clone(), data_dir: dir }
    }

    /// The `--profile` flag, then `RAVENGLASS_PROFILE`, then the platform's folders. Pure: the
    /// environment is an argument. `None` when no home folder is known.
    pub fn resolve(flag: Option<&str>, env: Env) -> Option<Profile> {
        let chosen = flag.map(str::to_owned).or_else(|| env(PROFILE_ENV)).filter(|dir| !dir.trim().is_empty());
        if let Some(dir) = chosen { return Some(Profile::at(dir)); }
        platform::dirs(env).map(|dirs| Profile { config_dir: dirs.config, data_dir: dirs.data })
    }

    pub fn config_file(&self) -> PathBuf { self.config_dir.join(CONFIG_FILE) }
    pub fn accounts_file(&self) -> PathBuf { self.config_dir.join(ACCOUNTS_FILE) }
    pub fn themes_dir(&self) -> PathBuf { self.config_dir.join(THEMES_DIR) }
    pub fn store_file(&self) -> PathBuf { self.data_dir.join(STORE_FILE) }

    /// Read the profile. Missing files are defaults; broken ones are reported and left alone.
    pub fn load(&self) -> Loaded {
        let mut out = Loaded::default();
        match read_capped(&self.config_file()) {
            Ok(Some(src)) => match Config::parse(&src) {
                Ok(config) => out.config = config,
                Err(e) => out.problems.push(first_line(&e.to_string())),
            },
            Ok(None) => {}
            Err(e) => out.problems.push(e),
        }
        match read_capped(&self.accounts_file()).and_then(|src| src.map(|s| accounts_from_toml(&s)).transpose()) {
            Ok(accounts) => out.accounts = accounts.unwrap_or_default(),
            Err(e) => out.problems.push(e),
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(self.themes_dir()).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
        files.sort();
        // Two passes, so a theme can name another user theme as its base whatever the file order.
        for _ in 0..2 {
            for path in &files {
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                if out.themes.iter().any(|t| t.id == slug(&stem)) { continue; }
                if let Ok(Some(src)) = read_capped(path) && let Ok(theme) = parse_user_theme(&src, &stem, &out.themes) && !out.themes.iter().any(|t| t.id == theme.id) {
                    out.themes.push(theme);
                }
            }
        }
        for path in &files {
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let read = read_capped(path).and_then(|src| parse_user_theme(&src.unwrap_or_default(), &stem, &out.themes).map(|_| ()));
            if let Err(e) = read && !out.themes.iter().any(|t| t.id == slug(&stem)) { out.problems.push(format!("themes/{stem}.toml: {}", first_line(&e))); }
        }
        out
    }

    /// Make the profile's folders, private to the user where the platform can.
    pub fn ensure_dirs(&self) -> Result<(), String> {
        for dir in [&self.config_dir, &self.data_dir] {
            std::fs::create_dir_all(dir).map_err(|e| format!("the profile folder cannot be made: {}", e.kind()))?;
            platform::restrict(dir, true);
        }
        Ok(())
    }

    /// Make the folders and, if it is not there yet, an empty `mail.db` that only the user can
    /// read: SQLite gives its `-wal` and `-shm` files the database's own mode.
    pub fn prepare_store(&self) -> Result<(), String> {
        self.ensure_dirs()?;
        let path = self.store_file();
        if !path.exists() {
            std::fs::File::create(&path).map_err(|e| format!("mail.db: {}", e.kind()))?;
            platform::restrict(&path, false);
        }
        Ok(())
    }

    pub fn save_accounts(&self, accounts: &[AccountConfig]) -> Result<(), String> {
        self.ensure_dirs()?;
        write_private(&self.accounts_file(), &accounts_to_toml(accounts)?)
    }

    /// Write `themes/<id>.toml`. The id is a slug, so the file cannot land outside the folder.
    pub fn save_theme(&self, id: &str, toml: &str) -> Result<PathBuf, String> {
        let id = slug(id);
        if id.is_empty() { return Err("the theme needs a name".into()); }
        self.ensure_dirs()?;
        let dir = self.themes_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("themes: {}", e.kind()))?;
        let path = dir.join(format!("{id}.toml"));
        write_private(&path, toml)?;
        Ok(path)
    }

    /// Delete the mail cache (the last account was removed). Missing files are fine.
    pub fn remove_store(&self) -> Result<(), String> {
        let base = self.store_file();
        for suffix in ["", "-wal", "-shm"] {
            let mut name = base.clone().into_os_string();
            name.push(suffix);
            match std::fs::remove_file(PathBuf::from(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("mail.db: {}", e.kind())),
            }
        }
        Ok(())
    }
}

fn first_line(text: &str) -> String { text.lines().next().unwrap_or("").trim().to_owned() }

/// Write through a temporary file and a rename, so a crash never leaves half a settings file.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let fail = |e: std::io::Error| format!("{name}: {}", e.kind());
    let mut tmp = path.to_path_buf().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(fail)?;
    platform::restrict(&tmp, false);
    std::fs::rename(&tmp, path).map_err(fail)
}

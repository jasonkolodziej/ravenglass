//! The user's `config.toml` (architecture appendix A). Parsing is strict: an unknown key or a
//! value out of range is an error naming it, never silently ignored, and a missing key takes
//! its default. Account settings and secrets are not here; see `mail-auth`.
//!
//! Text in, value out: where the file lives is the platform layer's business.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config.toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("config.toml: {0}")]
    Invalid(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReaderDefault { Html, Text }

/// How HTML mail is shown in a dark theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HtmlDark { Paper, Darken }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteImages { Ask, Never, Always }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Ui {
    /// A theme id, or `system` to follow the OS appearance using `[ui.system]`.
    pub theme: String,
    pub system: SystemThemes,
    pub reader_default: ReaderDefault,
    pub html_dark: HtmlDark,
    /// 0 sends at once; at most 30.
    pub undo_send_seconds: u32,
    /// Play the wallpaper intro at launch.
    pub intro: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self { theme: "ravenglass-night".into(), system: SystemThemes::default(), reader_default: ReaderDefault::Html, html_dark: HtmlDark::Paper, undo_send_seconds: 10, intro: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SystemThemes { pub light: String, pub dark: String }

impl Default for SystemThemes {
    fn default() -> Self { Self { light: "ravenglass-frost".into(), dark: "ravenglass-night".into() } }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Privacy {
    pub remote_images: RemoteImages,
    /// Lock the app after this many idle minutes; 0 never locks on its own.
    pub lock_after_minutes: u32,
    /// Ask for the OS biometric or password prompt when the app starts.
    pub lock_on_launch: bool,
}

impl Default for Privacy {
    fn default() -> Self { Self { remote_images: RemoteImages::Ask, lock_after_minutes: 0, lock_on_launch: false } }
}

/// One slot of the workspace bar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    /// 1 to 9.
    pub slot: u8,
    /// `account:<id>`, `account:<id>/folder:<name>` or `search:<query>`.
    pub source: String,
    /// A pane layout such as `vsplit(list, hsplit(reader, compose))`; the default layout if absent.
    #[serde(default)]
    pub layout: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub ui: Ui,
    /// Action name to key or chord, overriding the built-in table.
    pub keys: BTreeMap<String, String>,
    #[serde(rename = "workspace")]
    pub workspaces: Vec<Workspace>,
    pub privacy: Privacy,
}

impl Config {
    pub fn parse(src: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(src)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let bad = |message: String| Err(ConfigError::Invalid(message));
        if self.ui.undo_send_seconds > 30 { return bad(format!("ui.undo_send_seconds is {}, the most is 30", self.ui.undo_send_seconds)); }
        if self.privacy.lock_after_minutes > 24 * 60 { return bad("privacy.lock_after_minutes is more than a day".into()); }
        let mut slots = [false; 10];
        for w in &self.workspaces {
            if !(1..=9).contains(&w.slot) { return bad(format!("workspace slot {} is not 1 to 9", w.slot)); }
            if std::mem::replace(&mut slots[w.slot as usize], true) { return bad(format!("workspace slot {} is defined twice", w.slot)); }
            if !["account:", "search:"].iter().any(|p| w.source.strip_prefix(p).is_some_and(|rest| !rest.is_empty())) {
                return bad(format!("workspace {} source `{}` must start with account: or search:", w.slot, w.source));
            }
        }
        for (action, key) in &self.keys {
            if action.is_empty() || key.trim().is_empty() { return bad(format!("keys.{action} has no key")); }
        }
        Ok(())
    }

    /// The theme to use for an OS appearance.
    pub fn theme_for(&self, system_dark: bool) -> &str {
        match (self.ui.theme.as_str(), system_dark) { ("system", true) => &self.ui.system.dark, ("system", false) => &self.ui.system.light, (theme, _) => theme }
    }

    /// The file as the settings page would save it.
    pub fn to_toml(&self) -> String { toml::to_string(self).unwrap_or_default() }
}

/// `src` (the text of `config.toml`, maybe empty) with `[ui].theme` set to `theme`, and nothing
/// else changed: comments, order, spacing and the comment after the old value are kept. The
/// result must still parse, so a file that is already broken is refused rather than rewritten.
pub fn with_theme(src: &str, theme: &str) -> Result<String, ConfigError> {
    Config::parse(src)?;
    let mut doc: toml_edit::DocumentMut = src.parse().map_err(|e: toml_edit::TomlError| ConfigError::Invalid(e.message().to_owned()))?;
    let ui = doc.entry("ui").or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(ui) = ui.as_table_like_mut() else { return Err(ConfigError::Invalid("ui is not a table".into())) };
    match ui.get_mut("theme").and_then(toml_edit::Item::as_value_mut) {
        Some(old) => {
            let decor = old.decor().clone();
            *old = toml_edit::Value::from(theme);
            *old.decor_mut() = decor;
        }
        None => { ui.insert("theme", toml_edit::value(theme)); }
    }
    let out = doc.to_string();
    Config::parse(&out)?;
    Ok(out)
}

#[cfg(test)]
mod tests;

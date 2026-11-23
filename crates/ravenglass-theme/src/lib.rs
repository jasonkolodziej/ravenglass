//! Themes as data (plan §4 "Default palette" and "User theme customization").
//!
//! Every colour the UI draws is one of 16 named palette colours plus `background`, `foreground`
//! and `cursor`. Views ask for a *role* (`mail.unread`); the role names a palette entry; the entry
//! is a colour. A user theme overrides any subset of `[palette]`, `[roles]` and `[ui]` on top of a
//! base theme, so two lines can be a valid theme.

mod color;
mod import;

pub use color::{Rgb, contrast, fix_contrast};
pub use import::{import, import_alacritty, import_ghostty, import_iterm2, import_kitty, import_windows_terminal};

use serde::Deserialize;
use std::collections::BTreeMap;

/// The 19 palette names, in display order.
pub const NAMES: [&str; 19] = [
    "background", "foreground", "cursor", "black", "gray", "silver", "white", "maroon", "red", "green",
    "lime", "olive", "yellow", "navy", "blue", "purple", "fuchsia", "teal", "aqua",
];

pub const NIGHT_TOML: &str = include_str!("../../../assets/themes/ravenglass-night.toml");
pub const FROST_TOML: &str = include_str!("../../../assets/themes/ravenglass-frost.toml");

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("invalid TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("unknown palette name `{0}`")]
    UnknownName(String),
    #[error("bad colour `{value}` for `{key}`")]
    BadColor { key: String, value: String },
    #[error("unknown base theme `{0}`")]
    UnknownBase(String),
    #[error("role `{role}` points at unknown palette name `{name}`")]
    BadRole { role: String, name: String },
}

/// The file format (also what user themes and exports use).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeFile {
    pub name: Option<String>,
    pub id: Option<String>,
    pub appearance: Option<String>,
    pub version: Option<u32>,
    pub base: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub palette: BTreeMap<String, String>,
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    #[serde(default)]
    pub ui: Ui,
}

/// Non-colour settings. All optional in files; resolved against the base.
#[derive(Debug, Default, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Ui {
    pub font: Option<String>,
    pub font_size: Option<f32>,
    pub line_height: Option<f32>,
    pub window_opacity: Option<f32>,
    pub window_blur: Option<f32>,
    pub wallpaper: Option<String>,
    pub wallpaper_role: Option<String>,
    pub wallpaper_opacity: Option<f32>,
    pub intro: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance { Dark, Light }

/// A fully resolved theme: every name has a colour, every role resolves.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,
    pub id: String,
    pub appearance: Appearance,
    pub palette: BTreeMap<String, Rgb>,
    pub roles: BTreeMap<String, String>,
    pub ui: Ui,
}

impl Theme {
    pub fn night() -> Theme { Self::parse(NIGHT_TOML, &[]).expect("built-in Night parses") }
    pub fn frost() -> Theme { Self::parse(FROST_TOML, &[]).expect("built-in Frost parses") }

    /// Built-in themes by id.
    pub fn builtin(id: &str) -> Option<Theme> {
        match id { "ravenglass-night" => Some(Self::night()), "ravenglass-frost" => Some(Self::frost()), _ => None }
    }

    /// Parse a theme file. `base` themes are looked up among built-ins and `extra`.
    pub fn parse(src: &str, extra: &[Theme]) -> Result<Theme, ThemeError> {
        let file: ThemeFile = toml::from_str(src)?;
        let mut t = match &file.base {
            Some(b) => Self::builtin(b).or_else(|| extra.iter().find(|t| &t.id == b).cloned()).ok_or_else(|| ThemeError::UnknownBase(b.clone()))?,
            None if file.palette.len() == NAMES.len() => Theme {
                name: String::new(), id: String::new(), appearance: Appearance::Dark,
                palette: BTreeMap::new(), roles: BTreeMap::new(), ui: Ui::default(),
            },
            None => Self::night(), // partial theme without a base layers on Night
        };
        for (k, v) in &file.palette {
            if !NAMES.contains(&k.as_str()) { return Err(ThemeError::UnknownName(k.clone())); }
            let c = Rgb::parse(v).ok_or_else(|| ThemeError::BadColor { key: k.clone(), value: v.clone() })?;
            t.palette.insert(k.clone(), c);
        }
        for (role, name) in &file.roles {
            if !NAMES.contains(&name.as_str()) { return Err(ThemeError::BadRole { role: role.clone(), name: name.clone() }); }
            t.roles.insert(role.clone(), name.clone());
        }
        merge_ui(&mut t.ui, &file.ui);
        if let Some(n) = file.name { t.name = n; }
        if let Some(i) = file.id { t.id = i; }
        if let Some(a) = file.appearance { t.appearance = if a == "light" { Appearance::Light } else { Appearance::Dark }; }
        Ok(t)
    }

    /// A palette entry by name.
    pub fn color(&self, name: &str) -> Rgb { self.palette.get(name).copied().unwrap_or(Rgb::MAGENTA) }

    /// What views call: role → name → colour. Unknown roles draw magenta so they're obvious in snapshots.
    pub fn role(&self, role: &str) -> Rgb { self.roles.get(role).map(|n| self.color(n)).unwrap_or(Rgb::MAGENTA) }

    /// Roles whose text colour fails `min` contrast against `background`, with their ratio.
    pub fn contrast_report(&self, min: f32) -> Vec<(String, f32)> {
        let bg = self.color("background");
        self.roles.iter()
            .filter(|(r, _)| is_text_role(r))
            .map(|(r, n)| (r.clone(), contrast(self.color(n), bg)))
            .filter(|(_, c)| *c < min)
            .collect()
    }
}

/// Roles drawn as text on the background (the wallpaper, chrome and selection fills are exempt).
fn is_text_role(role: &str) -> bool {
    !matches!(role, "ui.bg" | "ui.chrome" | "ui.border" | "ui.selection" | "wallpaper" | "glass.tint")
}

fn merge_ui(into: &mut Ui, from: &Ui) {
    macro_rules! m { ($($f:ident),*) => { $( if from.$f.is_some() { into.$f = from.$f.clone(); } )* } }
    m!(font, font_size, line_height, window_opacity, window_blur, wallpaper, wallpaper_role, wallpaper_opacity, intro);
}

#[cfg(test)]
mod tests;

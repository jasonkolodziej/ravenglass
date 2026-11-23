//! Terminal colour-scheme importers. ANSI 0–15 map onto the 16 names (plan §4, item 3).
//! Ghostty, Alacritty, Kitty, Windows Terminal and iTerm2; [`import`] picks the format from the
//! text. Every importer returns a Ravenglass theme TOML layered on a base theme, and rejects a
//! colour it cannot read instead of guessing. The reference IDE's YAML is not covered yet.

use crate::{Rgb, ThemeError};
use std::fmt::Write;

/// ANSI index → Ravenglass palette name (the reference IDE's order).
pub const ANSI: [&str; 16] = ["black", "maroon", "green", "olive", "navy", "purple", "teal", "silver",
                              "gray", "red", "lime", "yellow", "blue", "fuchsia", "aqua", "white"];

/// Ghostty theme syntax (`palette = N=#rrggbb`, `background = …`, `foreground = …`,
/// `cursor-color = …`) → a Ravenglass theme TOML layered on `base`.
pub fn import_ghostty(src: &str, name: &str, base: &str) -> Result<String, ThemeError> {
    let mut palette: Vec<(String, String)> = vec![];
    for line in src.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let entry = match k {
            "background" => Some(("background".to_owned(), v)),
            "foreground" => Some(("foreground".to_owned(), v)),
            "cursor-color" => Some(("cursor".to_owned(), v)),
            "palette" => v.split_once('=').and_then(|(i, c)| i.trim().parse::<usize>().ok().filter(|i| *i < 16).map(|i| (ANSI[i].to_owned(), c.trim()))),
            _ => None,
        };
        if let Some((key, val)) = entry {
            let val = if val.starts_with('#') { val.to_owned() } else { format!("#{val}") };
            Rgb::parse(&val).ok_or_else(|| ThemeError::BadColor { key: key.clone(), value: val.clone() })?;
            palette.push((key, val));
        }
    }
    Ok(emit(name, base, &palette))
}

/// Theme TOML from `(palette name, colour)` pairs. The name is user text, so it is escaped.
fn emit(name: &str, base: &str, palette: &[(String, String)]) -> String {
    let quoted = |s: &str| s.chars().filter(|c| !c.is_control()).collect::<String>().replace('\\', "\\\\").replace('"', "\\\"");
    let mut out = String::new();
    let _ = writeln!(out, "name = \"{}\"\nbase = \"{}\"\n\n[palette]", quoted(name), quoted(base));
    for (k, v) in palette { let _ = writeln!(out, "{k} = \"{v}\""); }
    out
}

/// Check a colour and normalise it to `#RRGGBB`.
fn colour(key: &str, value: &str) -> Result<(String, String), ThemeError> {
    let v = value.trim().trim_matches(['"', '\'']);
    let v = v.strip_prefix("0x").map_or_else(|| if v.starts_with('#') { v.to_owned() } else { format!("#{v}") }, |hex| format!("#{hex}"));
    let rgb = Rgb::parse(&v).ok_or_else(|| ThemeError::BadColor { key: key.to_owned(), value: value.to_owned() })?;
    Ok((key.to_owned(), rgb.hex()))
}

/// Kitty (`background #1e1e2e`, `color0 … color15`, `cursor`).
pub fn import_kitty(src: &str, name: &str, base: &str) -> Result<String, ThemeError> {
    let mut palette = vec![];
    for line in src.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((k, v)) = line.split_once(char::is_whitespace) else { continue };
        let key = match k {
            "background" | "foreground" | "cursor" => k,
            _ => match k.strip_prefix("color").and_then(|n| n.parse::<usize>().ok()) { Some(i) if i < 16 => ANSI[i], _ => continue },
        };
        // `cursor none` and similar keywords are not colours; skip them.
        if v.trim().starts_with('#') { palette.push(colour(key, v)?); }
    }
    Ok(emit(name, base, &palette))
}

/// The eight ANSI colour names terminals use, in ANSI order.
const TERMINAL_NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];

/// Alacritty (`[colors.primary]`, `[colors.cursor]`, `[colors.normal]`, `[colors.bright]`).
pub fn import_alacritty(src: &str, name: &str, base: &str) -> Result<String, ThemeError> {
    let doc: toml::Table = toml::from_str(src)?;
    let colors = doc.get("colors").and_then(|c| c.as_table());
    let get = |section: &str, key: &str| colors.and_then(|c| c.get(section)).and_then(|s| s.get(key)).and_then(|v| v.as_str());
    let mut palette = vec![];
    for (key, section, field) in [("background", "primary", "background"), ("foreground", "primary", "foreground"), ("cursor", "cursor", "cursor")] {
        // Alacritty allows `cursor = "CellForeground"`; only real colours are imported.
        if let Some(v) = get(section, field).filter(|v| v.starts_with('#') || v.starts_with("0x")) { palette.push(colour(key, v)?); }
    }
    for (offset, section) in [(0, "normal"), (8, "bright")] {
        for (i, terminal) in TERMINAL_NAMES.iter().enumerate() {
            if let Some(v) = get(section, terminal) { palette.push(colour(ANSI[offset + i], v)?); }
        }
    }
    Ok(emit(name, base, &palette))
}

/// One Windows Terminal scheme object (`"background"`, `"brightBlue"`, `"purple"`, …).
pub fn import_windows_terminal(src: &str, name: &str, base: &str) -> Result<String, ThemeError> {
    let doc: serde_json::Value = serde_json::from_str(src).map_err(|e| ThemeError::BadColor { key: "json".into(), value: e.to_string() })?;
    let get = |key: &str| doc.get(key).and_then(|v| v.as_str());
    let mut palette = vec![];
    for (key, field) in [("background", "background"), ("foreground", "foreground"), ("cursor", "cursorColor")] {
        if let Some(v) = get(field) { palette.push(colour(key, v)?); }
    }
    // Windows Terminal calls ANSI magenta "purple".
    let names = ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white"];
    for (i, n) in names.iter().enumerate() {
        if let Some(v) = get(n) { palette.push(colour(ANSI[i], v)?); }
        let bright = format!("bright{}{}", n[..1].to_ascii_uppercase(), &n[1..]);
        if let Some(v) = get(&bright) { palette.push(colour(ANSI[8 + i], v)?); }
    }
    Ok(emit(name, base, &palette))
}

/// iTerm2 `.itermcolors` (an XML property list of `Ansi N Color` dictionaries of 0–1 reals).
pub fn import_iterm2(src: &str, name: &str, base: &str) -> Result<String, ThemeError> {
    let bad = |what: &str| ThemeError::BadColor { key: "itermcolors".into(), value: what.to_owned() };
    let doc = roxmltree::Document::parse_with_options(src, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).map_err(|e| bad(&e.to_string()))?;
    let root = doc.descendants().find(|n| n.has_tag_name("dict")).ok_or_else(|| bad("no dict"))?;
    let mut palette = vec![];
    for (key, value) in pairs(root) {
        let target = match key.as_str() {
            "Background Color" => "background",
            "Foreground Color" => "foreground",
            "Cursor Color" => "cursor",
            k => match k.strip_prefix("Ansi ").and_then(|r| r.strip_suffix(" Color")).and_then(|n| n.parse::<usize>().ok()) { Some(i) if i < 16 => ANSI[i], _ => continue },
        };
        let parts = pairs(value);
        let component = |name: &str| -> Result<u8, ThemeError> {
            let v = parts.iter().find(|(k, _)| k == name).and_then(|(_, n)| n.text()).and_then(|t| t.trim().parse::<f32>().ok()).ok_or_else(|| bad(&key))?;
            Ok((v.clamp(0.0, 1.0) * 255.0).round() as u8)
        };
        let (r, g, b) = (component("Red Component")?, component("Green Component")?, component("Blue Component")?);
        palette.push((target.to_owned(), format!("#{r:02X}{g:02X}{b:02X}")));
    }
    Ok(emit(name, base, &palette))
}

/// Import a scheme in whichever supported format `src` is in.
pub fn import(src: &str, name: &str, base: &str) -> Result<String, ThemeError> {
    let head = src.trim_start();
    if head.starts_with("<?xml") || head.starts_with("<!DOCTYPE plist") || head.starts_with("<plist") { return import_iterm2(src, name, base); }
    if head.starts_with('{') { return import_windows_terminal(src, name, base); }
    if src.contains("[colors") { return import_alacritty(src, name, base); }
    if src.lines().any(|l| l.trim_start().starts_with("palette") && l.contains('=')) { return import_ghostty(src, name, base); }
    if src.lines().any(|l| { let l = l.trim_start(); l.starts_with("color0") || l.starts_with("background ") }) { return import_kitty(src, name, base); }
    import_ghostty(src, name, base)
}

/// The `<key>` / value pairs of a property-list `<dict>`.
fn pairs<'a, 'i>(dict: roxmltree::Node<'a, 'i>) -> Vec<(String, roxmltree::Node<'a, 'i>)> {
    let elements: Vec<_> = dict.children().filter(|n| n.is_element()).collect();
    elements.chunks(2).filter(|c| c.len() == 2 && c[0].has_tag_name("key")).map(|c| (c[0].text().unwrap_or("").to_owned(), c[1])).collect()
}

//! Terminal colour-scheme importers. ANSI 0–15 map onto the 16 names (plan §4, item 3).
//! Ghostty first; add Alacritty, iTerm2, Kitty, Windows Terminal and the reference IDE's YAML next.

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
    let mut out = String::new();
    let _ = writeln!(out, "name = \"{name}\"\nbase = \"{base}\"\n\n[palette]");
    for (k, v) in palette { let _ = writeln!(out, "{k} = \"{v}\""); }
    Ok(out)
}

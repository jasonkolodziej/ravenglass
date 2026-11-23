//! The `[keys]` table of `config.toml` (architecture appendix A): an action name to the key or
//! chord that runs it, over the built-in table of `keys.rs`. The result is checked for clashes
//! before it is installed; a table that clashes is refused whole and the built-in one stays.
//!
//! ```toml
//! [keys]
//! archive = "a"            # replaces the NORMAL-mode keys of "archive"
//! palette = "mod+k"        # one key with a modifier replaces the action's chords
//! "focus left" = "ctrl+w a"
//! ```

use crate::actions::{self, Action};
use crate::keys::{self, Binding};
use std::collections::BTreeMap;

/// `focus-left`, `focus_left` and `Focus Left` all name the catalog's `focus left`.
fn normal(name: &str) -> String { name.trim().to_lowercase().replace(['-', '_'], " ").split_whitespace().collect::<Vec<_>>().join(" ") }

/// The action a `[keys]` entry names: a command name from the catalog (`archive`, `focus left`).
pub fn action_named(name: &str) -> Option<Action> {
    let wanted = normal(name);
    actions::catalog().iter().find(|e| normal(e.name) == wanted).map(|e| e.action.clone())
}

/// The built-in table with `overrides` applied, or the first thing wrong with them.
pub fn build(overrides: &BTreeMap<String, String>) -> Result<Vec<Binding>, String> {
    let mut table = keys::default_table();
    for (name, value) in overrides {
        let action = action_named(name).ok_or_else(|| format!("keys.{name}: no action has that name"))?;
        let seq = keys::parse_seq(value).map_err(|e| format!("keys.{name}: {e}"))?;
        let Some(first) = seq.first() else { return Err(format!("keys.{name} has no key")) };
        // One key held with a modifier works in every mode; anything else is a NORMAL-mode key.
        let chord = seq.len() == 1 && (first.ctrl || first.cmd || first.alt);
        let at = match table.iter().position(|b| b.action == action) {
            Some(at) => at,
            None => {
                table.push(Binding { action, normal: Vec::new(), chords: Vec::new() });
                table.len() - 1
            }
        };
        if chord { table[at].chords = vec![seq]; } else { table[at].normal = vec![seq]; }
    }
    match keys::clashes_in(&table).first() {
        Some((a, b)) => Err(format!("keys: {a} clashes with {b}")),
        None => Ok(table),
    }
}

/// Install the table for `overrides`. On a problem the built-in table is installed and the
/// problem returned, for the status bar.
pub fn apply(overrides: &BTreeMap<String, String>) -> Option<String> {
    if overrides.is_empty() {
        keys::install(None);
        return None;
    }
    match build(overrides) {
        Ok(table) => {
            keys::install(Some(table));
            None
        }
        Err(problem) => {
            keys::install(None);
            Some(problem)
        }
    }
}

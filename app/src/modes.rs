//! The modal input machine (plan §4): NORMAL, VISUAL, SEARCH, the `:` command line, the palette
//! and COMPOSE. One `enum Mode` whose variants carry only what is valid in them, and one
//! [`step`] that returns what should happen instead of doing it.

use crate::actions::{self, Action};
use crate::keys::{self, K, KeyPress, Lookup};
use rune::machine::Effects;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `pending` holds the keys of an unfinished sequence (`g`, `ctrl+w`).
    Normal { pending: Vec<KeyPress> },
    /// Multi-select: `j`/`k` extend the range from the anchor the app set on entry.
    Visual { pending: Vec<KeyPress> },
    /// `/`: the query is typed in the status line and results stream into the list.
    Search { query: String },
    /// `:`: a vim-style command line in the status line.
    Command { line: String },
    /// The fuzzy palette over every action.
    Palette { query: String, selected: usize },
    /// A stub until the compose pane lands (M7): only `esc` leaves it.
    Compose,
}

impl Default for Mode {
    fn default() -> Self { Mode::Normal { pending: Vec::new() } }
}

impl Mode {
    /// The label of the status line's mode block.
    pub fn label(&self) -> &'static str {
        match self {
            Mode::Normal { .. } => "NORMAL",
            Mode::Visual { .. } => "VISUAL",
            Mode::Search { .. } => "SEARCH",
            Mode::Command { .. } | Mode::Palette { .. } => "COMMAND",
            Mode::Compose => "COMPOSE",
        }
    }

    /// The theme role that colours the mode block.
    pub fn role(&self) -> &'static str {
        match self {
            Mode::Normal { .. } => "mode.normal",
            Mode::Visual { .. } => "mode.visual",
            Mode::Search { .. } => "mode.search",
            Mode::Command { .. } | Mode::Palette { .. } => "mode.command",
            Mode::Compose => "mode.compose",
        }
    }

    /// Keys of an unfinished sequence, for the status line (`g`, `Ctrl+W`).
    pub fn pending(&self) -> &[KeyPress] {
        match self {
            Mode::Normal { pending } | Mode::Visual { pending } => pending,
            _ => &[],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Run an action from the key table.
    Act(Action),
    /// VISUAL was entered: anchor the range at the selection.
    AnchorSelection,
    /// VISUAL was left: drop the range.
    ClearSelection,
    /// The search query changed; search as it is typed.
    SearchChanged(String),
    /// Enter in SEARCH: keep the results and go back to NORMAL.
    SearchAccepted(String),
    /// Esc in SEARCH: drop the results.
    SearchCancelled,
    /// Enter on the command line.
    RunCommand(String),
    /// Enter in the palette: run the `selected` match of `query`.
    RunPalette { query: String, selected: usize },
}

/// Feed one key press. The key table decides what NORMAL and VISUAL keys mean; text-entry modes
/// take characters themselves and pass modified chords on to the table.
pub fn step(mode: &mut Mode, key: KeyPress) -> Effects<Effect> {
    match mode {
        Mode::Normal { pending } => {
            let Some(action) = resolve(pending, key) else { return Effects::none() };
            enter(mode, action)
        }
        Mode::Visual { pending } => {
            let Some(action) = resolve(pending, key) else { return Effects::none() };
            match action {
                Action::Back | Action::Visual => {
                    *mode = Mode::default();
                    Effects::one(Effect::ClearSelection)
                }
                Action::Search | Action::CommandLine | Action::Palette | Action::Compose => {
                    let mut out = Effects::one(Effect::ClearSelection);
                    out.extend(enter(mode, action));
                    out
                }
                action if action.ends_visual() => {
                    *mode = Mode::default();
                    let mut out = Effects::one(Effect::Act(action));
                    out.push(Effect::ClearSelection);
                    out
                }
                action => Effects::one(Effect::Act(action)),
            }
        }
        Mode::Search { query } => match key.key {
            K::Esc => {
                *mode = Mode::default();
                Effects::one(Effect::SearchCancelled)
            }
            K::Enter => {
                let accepted = std::mem::take(query);
                *mode = Mode::default();
                Effects::one(Effect::SearchAccepted(accepted))
            }
            K::Backspace if query.is_empty() => {
                *mode = Mode::default();
                Effects::one(Effect::SearchCancelled)
            }
            K::Backspace => {
                query.pop();
                Effects::one(Effect::SearchChanged(query.clone()))
            }
            _ => match key.typed() {
                Some(c) => {
                    query.push(c);
                    Effects::one(Effect::SearchChanged(query.clone()))
                }
                None => chord(mode, key),
            },
        },
        Mode::Command { line } => match key.key {
            K::Esc => leave(mode),
            K::Backspace if line.is_empty() => leave(mode),
            K::Backspace => {
                line.pop();
                Effects::none()
            }
            K::Enter => {
                let run = std::mem::take(line);
                *mode = Mode::default();
                if run.trim().is_empty() { Effects::none() } else { Effects::one(Effect::RunCommand(run)) }
            }
            _ => match key.typed() {
                Some(c) => {
                    line.push(c);
                    Effects::none()
                }
                None => chord(mode, key),
            },
        },
        Mode::Palette { query, selected } => {
            let count = actions::palette_matches(query).len();
            match key.key {
                K::Esc => leave(mode),
                K::Enter => {
                    let run = Effect::RunPalette { query: std::mem::take(query), selected: *selected };
                    *mode = Mode::default();
                    Effects::one(run)
                }
                K::Down | K::Tab => {
                    *selected = if count == 0 { 0 } else { (*selected + 1) % count };
                    Effects::none()
                }
                K::Up => {
                    *selected = if count == 0 { 0 } else { (*selected + count - 1) % count };
                    Effects::none()
                }
                K::Char('n') if key.ctrl => {
                    *selected = if count == 0 { 0 } else { (*selected + 1) % count };
                    Effects::none()
                }
                K::Char('p') if key.ctrl && !key.shift => {
                    *selected = if count == 0 { 0 } else { (*selected + count - 1) % count };
                    Effects::none()
                }
                K::Backspace => {
                    query.pop();
                    *selected = 0;
                    Effects::none()
                }
                _ => match key.typed() {
                    Some(c) => {
                        query.push(c);
                        *selected = 0;
                        Effects::none()
                    }
                    // The palette's own chord closes it.
                    None if keys::lookup_chord(&key) == Some(Action::Palette) => leave(mode),
                    None => Effects::none(),
                },
            }
        }
        Mode::Compose => match key.key {
            K::Esc => leave(mode),
            _ => chord(mode, key),
        },
    }
}

/// Adds `key` to an unfinished sequence and says what it now means, if anything yet.
fn resolve(pending: &mut Vec<KeyPress>, key: KeyPress) -> Option<Action> {
    // After `ctrl+w` the hand often still holds Ctrl for the next key: `ctrl+w ctrl+h` is `ctrl+w h`.
    let held = pending.last().is_some_and(|p| p.ctrl) && key.ctrl && !key.alt;
    let key = if held { KeyPress { ctrl: false, cmd: false, ..key } } else { key };
    pending.push(key);
    match keys::lookup(pending) {
        Lookup::Exact(action) => {
            pending.clear();
            Some(action)
        }
        Lookup::Prefix => None,
        Lookup::None => {
            // An unknown continuation drops the sequence; the key may start a new one.
            let retry = pending.len() > 1;
            pending.clear();
            if !retry { return None; }
            pending.push(key);
            match keys::lookup(pending) {
                Lookup::Exact(action) => {
                    pending.clear();
                    Some(action)
                }
                Lookup::Prefix => None,
                Lookup::None => {
                    pending.clear();
                    None
                }
            }
        }
    }
}

/// An action that changes the mode does so here; every other action is the app's to run.
fn enter(mode: &mut Mode, action: Action) -> Effects<Effect> {
    match action {
        Action::Search => {
            *mode = Mode::Search { query: String::new() };
            Effects::one(Effect::SearchChanged(String::new()))
        }
        Action::CommandLine => {
            *mode = Mode::Command { line: String::new() };
            Effects::none()
        }
        Action::Palette => {
            *mode = Mode::Palette { query: String::new(), selected: 0 };
            Effects::none()
        }
        Action::Visual => {
            *mode = Mode::Visual { pending: Vec::new() };
            Effects::one(Effect::AnchorSelection)
        }
        Action::Compose => {
            *mode = Mode::Compose;
            Effects::one(Effect::Act(Action::Compose))
        }
        action => Effects::one(Effect::Act(action)),
    }
}

fn leave(mode: &mut Mode) -> Effects<Effect> {
    *mode = Mode::default();
    Effects::none()
}

/// A modified chord pressed in a text-entry mode runs its action as it would in NORMAL.
fn chord(mode: &mut Mode, key: KeyPress) -> Effects<Effect> {
    let Some(action) = keys::lookup_chord(&key) else { return Effects::none() };
    let was_search = matches!(mode, Mode::Search { .. });
    *mode = Mode::default();
    let mut out = if was_search { Effects::one(Effect::SearchCancelled) } else { Effects::none() };
    out.extend(enter(mode, action));
    out
}

/// Programmatic mode changes (palette entries, `:` commands) go through the same transitions.
pub fn enter_action(mode: &mut Mode, action: Action) -> Effects<Effect> {
    *mode = Mode::default();
    enter(mode, action)
}

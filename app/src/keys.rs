//! The key table (plan §4 "Keys"): one list of bindings, NORMAL-mode keys first and chords
//! second. Menus, the palette's key hints and the mode machine all read this table.
//!
//! A binding is a sequence of key specs written as text: `"g g"`, `"ctrl+w h"`, `"mod+shift+p"`.
//! `mod` is the platform's primary modifier (⌘ on macOS, Ctrl elsewhere), as in Rune's `Chord`.

use crate::actions::Action;
use crate::panes::{Axis, Dir};
use rune::{Key, KeyEvent};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum K {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

/// One key going down, as the mode machine sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyPress {
    pub key: K,
    pub ctrl: bool,
    /// The primary modifier (`Mods::cmd`).
    pub cmd: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyPress {
    pub const fn named(key: K) -> KeyPress { KeyPress { key, ctrl: false, cmd: false, alt: false, shift: false } }
    pub const fn char(c: char) -> KeyPress { KeyPress::named(K::Char(c)) }

    pub fn from_event(ev: &KeyEvent) -> Option<KeyPress> {
        let m = ev.mods;
        let key = match &ev.key {
            Key::Text(s) => K::Char(s.chars().next()?),
            Key::Command(c) => K::Char(*c),
            Key::Enter => K::Enter,
            Key::Escape => K::Esc,
            Key::Backspace => K::Backspace,
            Key::Delete => K::Delete,
            Key::Tab => K::Tab,
            Key::Up => K::Up,
            Key::Down => K::Down,
            Key::Left => K::Left,
            Key::Right => K::Right,
            Key::Home => K::Home,
            Key::End => K::End,
            Key::PageUp => K::PageUp,
            Key::PageDown => K::PageDown,
            _ => return None,
        };
        let cmd = m.cmd || matches!(ev.key, Key::Command(_));
        Some(KeyPress { key, ctrl: m.ctrl, cmd, alt: m.alt, shift: m.shift })
    }

    /// The character this press types into a text line, if it types one.
    pub fn typed(&self) -> Option<char> {
        match self.key {
            K::Char(c) if !self.ctrl && !self.cmd && !c.is_control() => Some(c),
            _ => None,
        }
    }

    fn has_modifier(&self) -> bool { self.ctrl || self.cmd || self.alt }
}

/// One step of a binding, parsed from `"mod+shift+p"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub key: K,
    pub ctrl: bool,
    pub cmd: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Off macOS Ctrl is the primary modifier, so `ctrl+x` and `mod+x` are the same keys there.
const CTRL_IS_MOD: bool = !cfg!(target_os = "macos");

impl KeySpec {
    pub fn parse(s: &str) -> Result<KeySpec, String> {
        let mut spec = KeySpec { key: K::Esc, ctrl: false, cmd: false, alt: false, shift: false };
        let mut parts: Vec<&str> = s.split('+').collect();
        // "ctrl+w +" and a bare "+": the key itself is a plus.
        let key = if s.ends_with('+') { parts.truncate(parts.len().saturating_sub(2)); "+" } else { parts.pop().unwrap_or("") };
        for m in parts {
            match m {
                "mod" => spec.cmd = true,
                "ctrl" => spec.ctrl = true,
                "alt" => spec.alt = true,
                "shift" => spec.shift = true,
                other => return Err(format!("unknown modifier `{other}` in `{s}`")),
            }
        }
        spec.key = match key {
            "enter" => K::Enter,
            "esc" => K::Esc,
            "backspace" => K::Backspace,
            "delete" => K::Delete,
            "tab" => K::Tab,
            "up" => K::Up,
            "down" => K::Down,
            "left" => K::Left,
            "right" => K::Right,
            "home" => K::Home,
            "end" => K::End,
            "pageup" => K::PageUp,
            "pagedown" => K::PageDown,
            "space" => K::Char(' '),
            k if k.chars().count() == 1 => K::Char(k.chars().next().unwrap_or(' ')),
            other => return Err(format!("unknown key `{other}` in `{s}`")),
        };
        Ok(spec)
    }

    fn primary(&self) -> bool { self.cmd || (CTRL_IS_MOD && self.ctrl) }

    pub fn matches(&self, p: &KeyPress) -> bool {
        let modified = self.ctrl || self.cmd;
        let key = match (self.key, p.key) {
            (K::Char(a), K::Char(b)) if modified => a.eq_ignore_ascii_case(&b),
            (a, b) => a == b,
        };
        if !key || self.alt != p.alt { return false; }
        let mods = if CTRL_IS_MOD {
            self.primary() == (p.cmd || p.ctrl)
        } else {
            self.cmd == p.cmd && self.ctrl == p.ctrl
        };
        // A plain character carries its own shift (`G`, `#`).
        let shift = matches!(self.key, K::Char(_)) && !modified || self.shift == p.shift;
        mods && shift
    }

    /// Whether two specs are pressed with the same keys on this platform.
    pub fn same_keys(&self, o: &KeySpec) -> bool {
        let modified = self.ctrl || self.cmd || o.ctrl || o.cmd;
        let key = match (self.key, o.key) {
            (K::Char(a), K::Char(b)) if modified => a.eq_ignore_ascii_case(&b),
            (a, b) => a == b,
        };
        let mods = if CTRL_IS_MOD { self.primary() == o.primary() } else { self.cmd == o.cmd && self.ctrl == o.ctrl };
        key && mods && self.alt == o.alt && (self.shift == o.shift || !modified && matches!(self.key, K::Char(_)))
    }

    /// How the key hint reads: `Ctrl+Shift+P`, `g`, `Enter`.
    pub fn label(&self) -> String {
        let mut out = String::new();
        if self.ctrl && !(CTRL_IS_MOD && self.cmd) { out.push_str("Ctrl+"); }
        if self.cmd { out.push_str(if CTRL_IS_MOD { "Ctrl+" } else { "Cmd+" }); }
        if self.alt { out.push_str("Alt+"); }
        if self.shift { out.push_str("Shift+"); }
        let modified = self.ctrl || self.cmd || self.alt;
        match self.key {
            K::Char(' ') => out.push_str("Space"),
            K::Char(c) if modified => out.extend(c.to_uppercase()),
            K::Char(c) => out.push(c),
            K::Enter => out.push_str("Enter"),
            K::Esc => out.push_str("Esc"),
            K::Backspace => out.push_str("Backspace"),
            K::Delete => out.push_str("Delete"),
            K::Tab => out.push_str("Tab"),
            K::Up => out.push('↑'),
            K::Down => out.push('↓'),
            K::Left => out.push('←'),
            K::Right => out.push('→'),
            K::Home => out.push_str("Home"),
            K::End => out.push_str("End"),
            K::PageUp => out.push_str("PgUp"),
            K::PageDown => out.push_str("PgDn"),
        }
        out
    }
}

pub type Seq = Vec<KeySpec>;

pub fn parse_seq(s: &str) -> Result<Seq, String> { s.split_whitespace().map(KeySpec::parse).collect() }

pub fn seq_label(seq: &[KeySpec]) -> String { seq.iter().map(KeySpec::label).collect::<Vec<_>>().join(" ") }

/// One row of the table: an action, the keys that run it in NORMAL and VISUAL mode, and the
/// chords that run it in every mode.
#[derive(Clone, Debug)]
pub struct Binding {
    pub action: Action,
    pub normal: Vec<Seq>,
    pub chords: Vec<Seq>,
}

/// The source of the table, as text. Users will rebind these from the `[keys]` config table.
fn rows() -> Vec<(Action, &'static [&'static str], &'static [&'static str])> {
    use Action as A;
    let mut v: Vec<(Action, &'static [&'static str], &'static [&'static str])> = vec![
        (A::MoveDown, &["j", "down"], &[]),
        (A::MoveUp, &["k", "up"], &[]),
        (A::Open, &["enter"], &[]),
        (A::Back, &["esc"], &[]),
        (A::Top, &["g g"], &["mod+up"]),
        (A::Bottom, &["G"], &["mod+down"]),
        (A::Archive, &["e"], &["ctrl+mod+a"]),
        (A::Delete, &["#", "backspace"], &[]),
        (A::ToggleRead, &["U"], &["mod+shift+u"]),
        (A::ToggleFlag, &["s"], &["mod+shift+l"]),
        (A::Reply, &["r"], &["mod+r"]),
        (A::ReplyAll, &["R"], &["mod+shift+r"]),
        (A::Forward, &["f"], &["mod+shift+f"]),
        (A::Compose, &["c"], &["mod+n"]),
        (A::Send, &[], &["mod+shift+d"]),
        (A::Search, &["/"], &["mod+f"]),
        (A::CommandLine, &[":"], &[]),
        (A::Palette, &[], &["mod+shift+p"]),
        (A::MoveTo, &["m"], &["mod+ctrl+m"]),
        (A::GoToMailbox, &["g m"], &[]),
        (A::ToggleView, &["t"], &[]),
        (A::OpenQuotes, &["z o"], &[]),
        (A::LoadImages { always: false }, &["i"], &[]),
        (A::LoadImages { always: true }, &["I"], &[]),
        (A::ToggleThread, &["z a"], &[]),
        (A::Visual, &["v"], &[]),
        (A::Undo, &["u"], &["mod+z"]),
        (A::NextTab, &["g t"], &["ctrl+tab"]),
        (A::PrevTab, &["g T"], &["ctrl+shift+tab"]),
        (A::PaneFocus(Dir::Left), &["ctrl+w h"], &[]),
        (A::PaneFocus(Dir::Down), &["ctrl+w j"], &[]),
        (A::PaneFocus(Dir::Up), &["ctrl+w k"], &[]),
        (A::PaneFocus(Dir::Right), &["ctrl+w l"], &[]),
        (A::PaneSplit(Axis::Rows), &["ctrl+w s"], &[]),
        (A::PaneSplit(Axis::Cols), &["ctrl+w v"], &[]),
        (A::PaneClose, &["ctrl+w q"], &[]),
        (A::PaneTree, &["ctrl+w t"], &[]),
        (A::PaneResize(Dir::Left), &["ctrl+w <"], &[]),
        (A::PaneResize(Dir::Right), &["ctrl+w >"], &[]),
        (A::PaneResize(Dir::Up), &["ctrl+w -"], &[]),
        (A::PaneResize(Dir::Down), &["ctrl+w +"], &[]),
    ];
    const DIGITS: [(&[&str], &[&str]); 9] = [
        (&["1"], &["mod+1"]), (&["2"], &["mod+2"]), (&["3"], &["mod+3"]), (&["4"], &["mod+4"]), (&["5"], &["mod+5"]),
        (&["6"], &["mod+6"]), (&["7"], &["mod+7"]), (&["8"], &["mod+8"]), (&["9"], &["mod+9"]),
    ];
    for (i, (normal, chord)) in DIGITS.iter().enumerate() {
        v.push((A::Workspace(i as u8 + 1), normal, chord));
    }
    v
}

/// The built-in table, before the user's `[keys]`.
pub fn default_table() -> Vec<Binding> {
    let parse = |specs: &[&str]| specs.iter().filter_map(|s| parse_seq(s).ok()).collect::<Vec<Seq>>();
    rows().into_iter().map(|(action, normal, chords)| Binding { action, normal: parse(normal), chords: parse(chords) }).collect()
}

/// The table with the user's `[keys]` applied (`keymap::build` makes and checks it).
static CUSTOM: std::sync::RwLock<Option<&'static [Binding]>> = std::sync::RwLock::new(None);

/// Use `table` from now on; `None` goes back to the built-in one. A replaced table is leaked: it
/// is a few kilobytes and changes only when the config is read.
pub fn install(table: Option<Vec<Binding>>) {
    if let Ok(mut custom) = CUSTOM.write() { *custom = table.map(|t| &*Vec::leak(t)); }
}

pub fn table() -> &'static [Binding] {
    if let Ok(custom) = CUSTOM.read() && let Some(table) = *custom { return table; }
    static TABLE: OnceLock<Vec<Binding>> = OnceLock::new();
    TABLE.get_or_init(default_table)
}

/// Specs in the table's source that fail to parse (a test keeps this empty).
#[cfg(test)]
pub fn parse_errors() -> Vec<String> {
    rows().iter().flat_map(|(_, n, c)| n.iter().chain(c.iter())).filter_map(|s| parse_seq(s).err()).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Exact(Action),
    /// More keys are needed (`g`, `ctrl+w`).
    Prefix,
    None,
}

fn seq_matches(seq: &[KeySpec], keys: &[KeyPress]) -> bool {
    seq.len() == keys.len() && seq.iter().zip(keys).all(|(s, k)| s.matches(k))
}

fn seq_starts(seq: &[KeySpec], keys: &[KeyPress]) -> bool {
    seq.len() > keys.len() && seq.iter().zip(keys).all(|(s, k)| s.matches(k))
}

/// NORMAL and VISUAL mode: plain keys, sequences and chords.
pub fn lookup(keys: &[KeyPress]) -> Lookup {
    let mut prefix = false;
    for b in table() {
        for seq in b.normal.iter().chain(&b.chords) {
            if seq_matches(seq, keys) { return Lookup::Exact(b.action.clone()); }
            prefix |= seq_starts(seq, keys);
        }
    }
    if prefix { Lookup::Prefix } else { Lookup::None }
}

/// Text-entry modes: only chords, and only ones held with a modifier, so typing stays typing.
pub fn lookup_chord(key: &KeyPress) -> Option<Action> {
    if !key.has_modifier() { return None; }
    table().iter().find(|b| b.chords.iter().any(|seq| seq_matches(seq, std::slice::from_ref(key)))).map(|b| b.action.clone())
}

/// The key hint the palette shows for an action: its first NORMAL key, then its first chord.
pub fn hint(action: &Action) -> String {
    let Some(b) = table().iter().find(|b| &b.action == action) else { return String::new() };
    b.normal.first().into_iter().chain(b.chords.first()).map(|s| seq_label(s)).collect::<Vec<_>>().join("  ")
}

/// Pairs of bindings that can't both work: the same keys, or one a prefix of the other.
#[cfg(test)]
pub fn clashes() -> Vec<(String, String)> { clashes_in(table()) }

/// [`clashes`] for any table: `[keys]` from the config is checked with it before it is used.
pub fn clashes_in(table: &[Binding]) -> Vec<(String, String)> {
    let all: Vec<(&Action, &Seq)> = table.iter().flat_map(|b| b.normal.iter().chain(&b.chords).map(move |s| (&b.action, s))).collect();
    let mut out = Vec::new();
    for (i, (a_act, a)) in all.iter().enumerate() {
        for (b_act, b) in &all[i + 1..] {
            let n = a.len().min(b.len());
            if a.iter().zip(b.iter()).take(n).all(|(x, y)| x.same_keys(y)) {
                out.push((format!("{a_act:?} {}", seq_label(a)), format!("{b_act:?} {}", seq_label(b))));
            }
        }
    }
    out
}

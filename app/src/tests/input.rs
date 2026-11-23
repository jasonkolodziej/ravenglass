//! The key table, the mode machine and the action catalog.

use crate::actions::{self, Action};
use crate::keys::{self, K, KeyPress, KeySpec, Lookup};
use crate::modes::{self, Effect, Mode};
use crate::panes::{Axis, Dir};

fn ch(c: char) -> KeyPress { KeyPress::char(c) }
fn named(k: K) -> KeyPress { KeyPress::named(k) }
fn ctrl(c: char) -> KeyPress { KeyPress { ctrl: true, cmd: !cfg!(target_os = "macos"), ..KeyPress::char(c) } }
fn primary(c: char, shift: bool) -> KeyPress { KeyPress { cmd: true, ctrl: !cfg!(target_os = "macos"), shift, ..KeyPress::char(c) } }

/// Feed keys; collect every effect.
fn feed(mode: &mut Mode, keys: &[KeyPress]) -> Vec<Effect> {
    keys.iter().flat_map(|k| modes::step(mode, *k).into_iter().collect::<Vec<_>>()).collect()
}

fn typed(mode: &mut Mode, text: &str) -> Vec<Effect> { feed(mode, &text.chars().map(ch).collect::<Vec<_>>()) }

// ── keys ───────────────────────────────────────────────────────────────────────────────────

#[test]
fn every_binding_in_the_table_parses() {
    assert_eq!(keys::parse_errors(), Vec::<String>::new());
    assert!(keys::table().iter().all(|b| !b.normal.is_empty() || !b.chords.is_empty()));
}

#[test]
fn no_two_bindings_clash() {
    assert_eq!(keys::clashes(), Vec::<(String, String)>::new(), "same keys, or one binding a prefix of another");
}

#[test]
fn a_clash_would_be_found() {
    let a = KeySpec::parse("mod+shift+p").unwrap();
    assert!(a.same_keys(&KeySpec::parse("mod+shift+p").unwrap()));
    assert!(!a.same_keys(&KeySpec::parse("mod+p").unwrap()));
    assert!(!KeySpec::parse("g").unwrap().same_keys(&KeySpec::parse("G").unwrap()));
    // Off macOS Ctrl is the primary modifier, so these are one chord there.
    let same = KeySpec::parse("ctrl+b").unwrap().same_keys(&KeySpec::parse("mod+b").unwrap());
    assert_eq!(same, !cfg!(target_os = "macos"));
}

#[test]
fn the_plan_keys_reach_their_actions() {
    let one = |k: KeyPress| keys::lookup(&[k]);
    assert_eq!(one(ch('j')), Lookup::Exact(Action::MoveDown));
    assert_eq!(one(named(K::Down)), Lookup::Exact(Action::MoveDown));
    assert_eq!(one(ch('k')), Lookup::Exact(Action::MoveUp));
    assert_eq!(one(named(K::Enter)), Lookup::Exact(Action::Open));
    assert_eq!(one(named(K::Esc)), Lookup::Exact(Action::Back));
    assert_eq!(one(ch('G')), Lookup::Exact(Action::Bottom));
    assert_eq!(one(ch('e')), Lookup::Exact(Action::Archive));
    assert_eq!(one(ch('#')), Lookup::Exact(Action::Delete));
    assert_eq!(one(ch('U')), Lookup::Exact(Action::ToggleRead));
    assert_eq!(one(ch('s')), Lookup::Exact(Action::ToggleFlag));
    assert_eq!(one(ch('u')), Lookup::Exact(Action::Undo));
    assert_eq!(one(ch('/')), Lookup::Exact(Action::Search));
    assert_eq!(one(ch(':')), Lookup::Exact(Action::CommandLine));
    assert_eq!(one(ch('t')), Lookup::Exact(Action::ToggleView));
    assert_eq!(one(ch('v')), Lookup::Exact(Action::Visual));
    for n in 1..=9u8 {
        assert_eq!(one(ch(char::from(b'0' + n))), Lookup::Exact(Action::Workspace(n)));
    }
    assert_eq!(one(primary('p', true)), Lookup::Exact(Action::Palette));
    assert_eq!(one(primary('z', false)), Lookup::Exact(Action::Undo));
    assert_eq!(one(ch('x')), Lookup::None);
}

#[test]
fn sequences_wait_for_their_second_key() {
    assert_eq!(keys::lookup(&[ch('g')]), Lookup::Prefix);
    assert_eq!(keys::lookup(&[ch('g'), ch('g')]), Lookup::Exact(Action::Top));
    assert_eq!(keys::lookup(&[ch('g'), ch('t')]), Lookup::Exact(Action::NextTab));
    assert_eq!(keys::lookup(&[ch('g'), ch('T')]), Lookup::Exact(Action::PrevTab));
    assert_eq!(keys::lookup(&[ctrl('w')]), Lookup::Prefix);
    assert_eq!(keys::lookup(&[ctrl('w'), ch('h')]), Lookup::Exact(Action::PaneFocus(Dir::Left)));
    assert_eq!(keys::lookup(&[ctrl('w'), ch('s')]), Lookup::Exact(Action::PaneSplit(Axis::Rows)));
    assert_eq!(keys::lookup(&[ctrl('w'), ch('v')]), Lookup::Exact(Action::PaneSplit(Axis::Cols)));
    assert_eq!(keys::lookup(&[ctrl('w'), ch('q')]), Lookup::Exact(Action::PaneClose));
    assert_eq!(keys::lookup(&[ctrl('w'), ch('+')]), Lookup::Exact(Action::PaneResize(Dir::Down)));
    assert_eq!(keys::lookup(&[ch('w')]), Lookup::None, "a plain w is not ctrl+w");
}

#[test]
fn typing_is_never_a_chord() {
    assert_eq!(keys::lookup_chord(&ch('e')), None);
    assert_eq!(keys::lookup_chord(&primary('p', true)), Some(Action::Palette));
    assert_eq!(ch('e').typed(), Some('e'));
    assert_eq!(primary('p', true).typed(), None);
}

#[test]
fn hints_come_from_the_table() {
    assert!(keys::hint(&Action::Archive).starts_with('e'));
    assert!(keys::hint(&Action::Palette).ends_with("Shift+P"));
    assert_eq!(keys::hint(&Action::PaneFocus(Dir::Left)), "Ctrl+W h");
    assert_eq!(keys::hint(&Action::Quit), "");
}

// ── modes ──────────────────────────────────────────────────────────────────────────────────

#[test]
fn normal_keys_become_actions() {
    let mut mode = Mode::default();
    assert_eq!(feed(&mut mode, &[ch('j'), ch('k'), ch('e')]), [Effect::Act(Action::MoveDown), Effect::Act(Action::MoveUp), Effect::Act(Action::Archive)]);
    assert_eq!(mode, Mode::default());
    assert_eq!(mode.label(), "NORMAL");
}

#[test]
fn a_sequence_is_pending_until_it_resolves() {
    let mut mode = Mode::default();
    assert!(feed(&mut mode, &[ch('g')]).is_empty());
    assert_eq!(mode.pending(), [ch('g')]);
    assert_eq!(feed(&mut mode, &[ch('g')]), [Effect::Act(Action::Top)]);
    assert!(mode.pending().is_empty());
    // An unknown continuation drops the sequence, and the key starts over.
    assert_eq!(feed(&mut mode, &[ch('g'), ch('j')]), [Effect::Act(Action::MoveDown)]);
    assert!(feed(&mut mode, &[ch('g'), ch('x')]).is_empty());
    assert!(mode.pending().is_empty());
    assert_eq!(feed(&mut mode, &[ctrl('w'), ch('l')]), [Effect::Act(Action::PaneFocus(Dir::Right))]);
    assert_eq!(feed(&mut mode, &[ctrl('w'), ctrl('h')]), [Effect::Act(Action::PaneFocus(Dir::Left))], "Ctrl may stay held");
}

#[test]
fn the_command_line_collects_a_line_and_runs_it() {
    let mut mode = Mode::default();
    assert!(typed(&mut mode, ":theme frost").is_empty());
    assert_eq!(mode, Mode::Command { line: "theme frost".into() });
    assert_eq!(mode.label(), "COMMAND");
    assert!(feed(&mut mode, &[named(K::Backspace)]).is_empty());
    assert_eq!(mode, Mode::Command { line: "theme fros".into() });
    assert_eq!(feed(&mut mode, &[ch('t'), named(K::Enter)]), [Effect::RunCommand("theme frost".into())]);
    assert_eq!(mode, Mode::default());
}

#[test]
fn escape_and_an_empty_backspace_leave_the_command_line() {
    let mut mode = Mode::default();
    typed(&mut mode, ":ar");
    assert!(feed(&mut mode, &[named(K::Esc)]).is_empty());
    assert_eq!(mode, Mode::default());
    typed(&mut mode, ":");
    feed(&mut mode, &[named(K::Backspace)]);
    assert_eq!(mode, Mode::default());
    typed(&mut mode, ":");
    assert!(feed(&mut mode, &[named(K::Enter)]).is_empty(), "an empty line runs nothing");
    assert_eq!(mode, Mode::default());
}

#[test]
fn keys_typed_on_the_command_line_are_text_not_actions() {
    let mut mode = Mode::default();
    assert!(typed(&mut mode, ":jke#u").is_empty());
    assert_eq!(mode, Mode::Command { line: "jke#u".into() });
}

#[test]
fn search_streams_its_query_and_keeps_or_drops_the_results() {
    let mut mode = Mode::default();
    assert_eq!(typed(&mut mode, "/is"), [Effect::SearchChanged(String::new()), Effect::SearchChanged("i".into()), Effect::SearchChanged("is".into())]);
    assert_eq!(mode.label(), "SEARCH");
    assert_eq!(feed(&mut mode, &[named(K::Enter)]), [Effect::SearchAccepted("is".into())]);
    assert_eq!(mode, Mode::default());
    typed(&mut mode, "/x");
    assert_eq!(feed(&mut mode, &[named(K::Esc)]), [Effect::SearchCancelled]);
    assert_eq!(mode, Mode::default());
}

#[test]
fn visual_anchors_extends_and_ends_on_an_action() {
    let mut mode = Mode::default();
    assert_eq!(feed(&mut mode, &[ch('v')]), [Effect::AnchorSelection]);
    assert_eq!(mode.label(), "VISUAL");
    assert_eq!(feed(&mut mode, &[ch('j'), ch('j')]), [Effect::Act(Action::MoveDown), Effect::Act(Action::MoveDown)]);
    assert_eq!(mode.label(), "VISUAL", "moving keeps the mode");
    assert_eq!(feed(&mut mode, &[ch('e')]), [Effect::Act(Action::Archive), Effect::ClearSelection]);
    assert_eq!(mode, Mode::default());
    feed(&mut mode, &[ch('v')]);
    assert_eq!(feed(&mut mode, &[named(K::Esc)]), [Effect::ClearSelection]);
    assert_eq!(mode, Mode::default());
}

#[test]
fn the_palette_filters_moves_and_runs() {
    let mut mode = Mode::default();
    assert!(feed(&mut mode, &[primary('p', true)]).is_empty());
    assert_eq!(mode, Mode::Palette { query: String::new(), selected: 0 });
    typed(&mut mode, "pane");
    let count = actions::palette_matches("pane").len();
    assert!(count > 3);
    feed(&mut mode, &[named(K::Down), named(K::Down)]);
    assert_eq!(mode, Mode::Palette { query: "pane".into(), selected: 2 });
    feed(&mut mode, &[named(K::Up), named(K::Up), named(K::Up)]);
    assert_eq!(mode, Mode::Palette { query: "pane".into(), selected: count - 1 }, "the selection wraps");
    typed(&mut mode, "x");
    assert_eq!(mode, Mode::Palette { query: "panex".into(), selected: 0 }, "typing starts from the first match");
    feed(&mut mode, &[named(K::Backspace)]);
    assert_eq!(feed(&mut mode, &[named(K::Enter)]), [Effect::RunPalette { query: "pane".into(), selected: 0 }]);
    assert_eq!(mode, Mode::default());
}

#[test]
fn the_palette_chord_works_from_text_modes_and_closes_itself() {
    let mut mode = Mode::default();
    typed(&mut mode, ":th");
    feed(&mut mode, &[primary('p', true)]);
    assert!(matches!(mode, Mode::Palette { .. }));
    feed(&mut mode, &[primary('p', true)]);
    assert_eq!(mode, Mode::default());
    typed(&mut mode, "/q");
    assert_eq!(feed(&mut mode, &[primary('p', true)]), [Effect::SearchCancelled]);
    assert!(matches!(mode, Mode::Palette { .. }));
}

#[test]
fn compose_leaves_typing_to_the_field_and_keeps_esc_tab_and_send() {
    let mut mode = Mode::default();
    assert_eq!(feed(&mut mode, &[ch('c')]), [Effect::Act(Action::Compose)]);
    assert_eq!(mode.label(), "COMPOSE");
    assert!(feed(&mut mode, &[ch('j'), ch(':'), named(K::Enter), named(K::Backspace)]).is_empty(), "typing is the focused field's");
    assert_eq!(feed(&mut mode, &[named(K::Tab)]), [Effect::ComposeField { back: false }]);
    assert_eq!(feed(&mut mode, &[KeyPress { shift: true, ..named(K::Tab) }]), [Effect::ComposeField { back: true }]);
    // The send chord acts without leaving; undo and the other editing chords stay the editor's.
    assert_eq!(feed(&mut mode, &[primary('d', true)]), [Effect::Act(Action::Send)]);
    assert!(feed(&mut mode, &[primary('z', false), primary('a', false), primary('r', false)]).is_empty());
    assert_eq!(mode, Mode::Compose);
    feed(&mut mode, &[named(K::Esc)]);
    assert_eq!(mode, Mode::default());
    // The palette and workspace chords leave the draft as it is.
    mode = Mode::Compose;
    feed(&mut mode, &[primary('p', true)]);
    assert!(matches!(mode, Mode::Palette { .. }));
    mode = Mode::Compose;
    assert_eq!(feed(&mut mode, &[primary('2', false)]), [Effect::Act(Action::Workspace(2))]);
    assert_eq!(mode, Mode::default());
    // The reply keys and the commands that end a draft.
    assert_eq!(typed(&mut mode, "rRf"), [Effect::Act(Action::Reply), Effect::Act(Action::ReplyAll), Effect::Act(Action::Forward)]);
    assert_eq!(actions::parse_command(":send"), Ok(Action::Send));
    assert_eq!(actions::parse_command(":discard"), Ok(Action::Discard));
    assert_eq!(actions::parse_command(":retry"), Ok(Action::Retry));
}

// ── actions ────────────────────────────────────────────────────────────────────────────────

#[test]
fn commands_parse_to_the_actions_keys_run() {
    assert_eq!(actions::parse_command(":archive"), Ok(Action::Archive));
    assert_eq!(actions::parse_command("archive"), Ok(Action::Archive));
    assert_eq!(actions::parse_command("theme Frost"), Ok(Action::Theme("frost".into())));
    assert_eq!(actions::parse_command("search from:github is:unread"), Ok(Action::SearchFor("from:github is:unread".into())));
    assert_eq!(actions::parse_command("mailbox Receipts"), Ok(Action::Mailbox("Receipts".into())));
    assert_eq!(actions::parse_command("workspace 2"), Ok(Action::Workspace(2)));
    assert_eq!(actions::parse_command("vsplit"), Ok(Action::PaneSplit(Axis::Cols)));
    assert_eq!(actions::parse_command("focus   left"), Ok(Action::PaneFocus(Dir::Left)));
    assert_eq!(actions::parse_command("q"), Ok(Action::Quit));
    assert!(actions::parse_command("theme").is_err());
    assert!(actions::parse_command("workspace 0").is_err());
    assert!(actions::parse_command("frobnicate").is_err());
    assert!(actions::parse_command("  ").is_err());
}

#[test]
fn every_catalog_entry_round_trips_through_the_command_line() {
    for entry in actions::catalog() {
        assert_eq!(actions::parse_command(entry.name).as_ref(), Ok(&entry.action), ":{}", entry.name);
    }
    let mut names: Vec<&str> = actions::catalog().iter().map(|e| e.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), actions::catalog().len(), "command names are unique");
}

#[test]
fn every_bound_action_is_in_the_palette() {
    for binding in keys::table() {
        assert!(actions::catalog().iter().any(|e| e.action == binding.action), "{:?} has keys but no palette entry", binding.action);
    }
}

#[test]
fn the_palette_matches_fuzzily_best_first() {
    assert_eq!(actions::palette_matches("").len(), actions::catalog().len());
    assert_eq!(actions::palette_matches("arch")[0].action, Action::Archive);
    assert_eq!(actions::palette_matches("thfr")[0].action, Action::Theme("frost".into()));
    assert!(actions::palette_matches("zzzz").is_empty());
}

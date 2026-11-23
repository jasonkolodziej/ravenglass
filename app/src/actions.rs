//! What keys, the command line and the palette all dispatch (plan §4 "Keys"): one `Action` enum,
//! one catalog that names each action for `:` and for the palette.

use crate::panes::{Axis, Dir, Kind};
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    MoveDown,
    MoveUp,
    Top,
    Bottom,
    Open,
    Back,
    Archive,
    Delete,
    ToggleRead,
    ToggleFlag,
    Undo,
    Search,
    CommandLine,
    Palette,
    Visual,
    ToggleView,
    OpenQuotes,
    /// Load the open message's remote images: this once, or always for its sender.
    LoadImages { always: bool },
    ToggleThread,
    Workspace(u8),
    NextTab,
    PrevTab,
    PaneFocus(Dir),
    PaneSplit(Axis),
    PaneClose,
    PaneResize(Dir),
    PaneTree,
    PaneShow(Kind),
    Reply,
    ReplyAll,
    Forward,
    Compose,
    Send,
    MoveTo,
    GoToMailbox,
    Theme(String),
    SearchFor(String),
    Mailbox(String),
    Refresh,
    Quit,
    /// Start another draft even when one is open (`c` goes back to the open one).
    NewDraft,
    Discard,
    /// Send a failed message again.
    Retry,
}

impl Action {
    /// Acts on the selection, so VISUAL mode ends after it.
    pub fn ends_visual(&self) -> bool {
        matches!(self, Action::Archive | Action::Delete | Action::ToggleRead | Action::ToggleFlag | Action::Back | Action::Open)
    }
}

/// One nameable action: `name` is what `:` takes, `title` what the palette lists.
#[derive(Clone, Debug)]
pub struct Entry {
    pub action: Action,
    pub name: &'static str,
    pub title: &'static str,
}

/// Every action with a fixed name, in palette order. Actions that take free text (`:search q`,
/// `:mailbox name`) are parsed by [`parse_command`] instead.
pub fn catalog() -> &'static [Entry] {
    static CATALOG: OnceLock<Vec<Entry>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let e = |action, name, title| Entry { action, name, title };
        let mut v = vec![
            e(Action::Archive, "archive", "Archive"),
            e(Action::Delete, "delete", "Delete"),
            e(Action::ToggleRead, "read", "Toggle read"),
            e(Action::ToggleFlag, "flag", "Toggle flag"),
            e(Action::Undo, "undo", "Undo"),
            e(Action::Open, "open", "Open message"),
            e(Action::Back, "back", "Back"),
            e(Action::Search, "find", "Search"),
            e(Action::CommandLine, "command", "Command line"),
            e(Action::Palette, "palette", "Command palette"),
            e(Action::Visual, "visual", "Visual select"),
            e(Action::ToggleView, "view", "Toggle HTML / text view"),
            e(Action::OpenQuotes, "quotes", "Show quoted text"),
            e(Action::LoadImages { always: false }, "images", "Load remote images once"),
            e(Action::LoadImages { always: true }, "images-always", "Always load images from this sender"),
            e(Action::ToggleThread, "thread", "Toggle thread"),
            e(Action::Reply, "reply", "Reply"),
            e(Action::ReplyAll, "replyall", "Reply all"),
            e(Action::Forward, "forward", "Forward"),
            e(Action::Compose, "compose", "Compose"),
            e(Action::Send, "send", "Send"),
            e(Action::MoveTo, "move", "Move to mailbox"),
            e(Action::GoToMailbox, "goto", "Go to mailbox"),
            e(Action::MoveDown, "next", "Next message"),
            e(Action::MoveUp, "prev", "Previous message"),
            e(Action::Top, "top", "First message"),
            e(Action::Bottom, "bottom", "Last message"),
            e(Action::NextTab, "tabnext", "Next tab"),
            e(Action::PrevTab, "tabprev", "Previous tab"),
            e(Action::PaneSplit(Axis::Rows), "split", "Pane: split below"),
            e(Action::PaneSplit(Axis::Cols), "vsplit", "Pane: split right"),
            e(Action::PaneClose, "close", "Pane: close"),
            e(Action::PaneFocus(Dir::Left), "focus left", "Pane: focus left"),
            e(Action::PaneFocus(Dir::Right), "focus right", "Pane: focus right"),
            e(Action::PaneFocus(Dir::Up), "focus up", "Pane: focus up"),
            e(Action::PaneFocus(Dir::Down), "focus down", "Pane: focus down"),
            e(Action::PaneResize(Dir::Left), "resize left", "Pane: narrower"),
            e(Action::PaneResize(Dir::Right), "resize right", "Pane: wider"),
            e(Action::PaneResize(Dir::Up), "resize up", "Pane: shorter"),
            e(Action::PaneResize(Dir::Down), "resize down", "Pane: taller"),
            e(Action::PaneTree, "tree", "Pane: mailbox tree"),
            e(Action::PaneShow(Kind::List), "pane list", "Pane: show message list"),
            e(Action::PaneShow(Kind::Reader), "pane reader", "Pane: show reader"),
            e(Action::PaneShow(Kind::Empty), "pane empty", "Pane: show nothing"),
            e(Action::Theme("night".into()), "theme night", "Theme: Night"),
            e(Action::Theme("frost".into()), "theme frost", "Theme: Frost"),
            e(Action::Refresh, "refresh", "Check for new mail"),
            e(Action::Quit, "quit", "Quit"),
            e(Action::NewDraft, "new", "New message (another draft)"),
            e(Action::Discard, "discard", "Discard draft"),
            e(Action::Retry, "retry", "Retry sending"),
            e(Action::PaneShow(Kind::Compose), "pane compose", "Pane: show draft"),
        ];
        const WORKSPACES: [(&str, &str); 9] = [
            ("workspace 1", "Workspace 1"), ("workspace 2", "Workspace 2"), ("workspace 3", "Workspace 3"),
            ("workspace 4", "Workspace 4"), ("workspace 5", "Workspace 5"), ("workspace 6", "Workspace 6"),
            ("workspace 7", "Workspace 7"), ("workspace 8", "Workspace 8"), ("workspace 9", "Workspace 9"),
        ];
        for (i, (name, title)) in WORKSPACES.iter().enumerate() {
            v.push(e(Action::Workspace(i as u8 + 1), name, title));
        }
        v
    })
}

/// A `:` line to its action. The leading `:` is optional.
pub fn parse_command(line: &str) -> Result<Action, String> {
    let line = line.trim().trim_start_matches(':').trim();
    if line.is_empty() { return Err("empty command".into()); }
    let (head, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(h, r)| (h, r.trim()));
    let need = |what: &str| if rest.is_empty() { Err(format!(":{head} needs {what}")) } else { Ok(rest.to_owned()) };
    match head {
        "search" | "s" => return need("a query").map(Action::SearchFor),
        "mailbox" | "mb" => return need("a mailbox name").map(Action::Mailbox),
        "theme" => return need("a theme name").map(|n| Action::Theme(n.to_lowercase())),
        "workspace" | "ws" => {
            return match rest.parse::<u8>() {
                Ok(n @ 1..=9) => Ok(Action::Workspace(n)),
                _ => Err(":workspace needs a number 1–9".into()),
            };
        }
        "q" => return Ok(Action::Quit),
        "sp" => return Ok(Action::PaneSplit(Axis::Rows)),
        "vs" => return Ok(Action::PaneSplit(Axis::Cols)),
        // `:move Receipts` names its target; the picker itself is not built yet.
        "move" => return Ok(Action::MoveTo),
        _ => {}
    }
    let wanted = line.split_whitespace().collect::<Vec<_>>().join(" ");
    catalog().iter().find(|e| e.name == wanted).map(|e| e.action.clone()).ok_or_else(|| format!("unknown command: {head}"))
}

/// Catalog entries matching a palette query: every query character in order, case-insensitive.
/// Earlier and tighter matches come first; the catalog's own order breaks ties.
pub fn palette_matches(query: &str) -> Vec<&'static Entry> {
    let q: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let mut scored: Vec<(usize, usize, &'static Entry)> = catalog()
        .iter()
        .enumerate()
        .filter_map(|(i, e)| fuzzy_score(&q, e.title).map(|score| (score, i, e)))
        .collect();
    scored.sort_by_key(|(score, i, _)| (*score, *i));
    scored.into_iter().map(|(_, _, e)| e).collect()
}

/// Lower is better; `None` when `q` is not a subsequence of `title`.
fn fuzzy_score(q: &[char], title: &str) -> Option<usize> {
    let mut score = 0;
    let mut want = q.iter().peekable();
    let mut last: Option<usize> = None;
    for (i, c) in title.to_lowercase().chars().enumerate() {
        let Some(w) = want.peek() else { break };
        if c == **w {
            score += match last { Some(l) => i - l - 1, None => i };
            last = Some(i);
            want.next();
        }
    }
    want.peek().is_none().then_some(score)
}

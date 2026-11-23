//! The bottom workspace bar: numbered slots on the left (`1`–`9` switch), sync state on the right.

use super::line::{Line, Mark};
use super::{BAR_ROWS, Ui};
use crate::grid;
use crate::state::AppState;
use crate::timefmt;
use mail_core::machine::account_sync::State as Sync;
use rune::{El, el};

/// One line for all accounts: the least settled state wins.
pub fn sync_text(s: &AppState) -> String {
    if s.link.is_off() { return "offline".into(); }
    let states: Vec<&Sync> = s.mail.sync.values().collect();
    if states.is_empty() { return "starting".into(); }
    if states.iter().any(|st| matches!(st, Sync::AuthFailed)) { return "sign-in failed".into(); }
    if states.iter().any(|st| matches!(st, Sync::Backoff { .. })) { return "offline · retrying".into(); }
    if let Some(Sync::Syncing { done, total }) = states.iter().find(|st| matches!(st, Sync::Syncing { .. })) {
        return format!("syncing {done}/{total}");
    }
    if states.iter().any(|st| matches!(st, Sync::Connecting { .. } | Sync::Authenticating { .. } | Sync::Disconnected)) {
        return "connecting".into();
    }
    match s.mail.synced_at {
        Some(at) => format!("synced {} · IDLE", timefmt::clock(at)),
        None => "IDLE".into(),
    }
}

pub fn bar(s: &AppState, ui: &Ui, cols: usize) -> El {
    let g = &ui.g;
    let mut bar = el().key("workspaces").role("tablist").size(g.x(cols), g.y(BAR_ROWS));
    let y = g.cell_h / 2.0;
    let state = sync_text(s);
    let state_cols = grid::width(&state) + 4;
    let mut col = 1;
    for (i, ws) in s.workspaces.iter().enumerate() {
        let active = i == s.active;
        let width = grid::width(&ws.name) + 2;
        if col + width + state_cols > cols { break; }
        let slot = ws.slot;
        let (digit, name) = if active { (ui.p.accent, ui.p.text) } else { (ui.p.muted, ui.p.muted) };
        let item = Line::new().put(&slot.to_string(), digit).gap(1).put(&ws.name, name).el(ui, width)
            .key(format!("ws:{slot}")).role("tab").label(format!("Workspace {slot}: {}", ws.name)).checked(active)
            .absolute(g.x(col), y)
            .on_press(move |s: &mut AppState| s.perform(crate::actions::Action::Workspace(slot)));
        bar = bar.child(item);
        col += width + 2;
    }
    if state_cols < cols {
        let line = Line::new().mark(Mark::Sync, ui.p.muted).gap(1).put(&state, ui.p.muted).el(ui, state_cols - 2);
        bar = bar.child(line.key("sync").label(state).absolute(g.x(cols - state_cols), y));
    }
    bar
}

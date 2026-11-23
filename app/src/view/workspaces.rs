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
    if let Some(line) = s.shell.status_line() { return line; }
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
            .on_press_tx(move |s: &mut AppState, tx| {
                s.clock = tx.clock();
                s.perform(crate::actions::Action::Workspace(slot));
                crate::flush(s, tx);
            });
        bar = bar.child(item);
        col += width + 2;
    }
    let mut right = cols;
    if state_cols < cols {
        let line = Line::new().mark(Mark::Sync, ui.p.muted).gap(1).put(&state, ui.p.muted).el(ui, state_cols - 2);
        right = cols - state_cols;
        bar = bar.child(line.key("sync").label(state).absolute(g.x(right), y));
    }
    // The toast takes the room between the slots and the sync state.
    if let Some(toast) = &s.toast {
        let room = right.saturating_sub(col + 2);
        if room >= 12 {
            let width = super::toast::width(toast, room);
            bar = bar.child(super::toast::toast(ui, toast, room).absolute(g.x(right - width - 2), y));
        }
    }
    bar
}

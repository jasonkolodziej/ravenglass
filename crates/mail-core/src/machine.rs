//! Explicit state machines (plan §2 "Data flow"; Rune AGENTS.md "State"): one `enum State` whose
//! variants carry only the data valid in them, and one `step` that returns effects instead of
//! performing them. Timers are events: a timed variant stores its due time and the caller wakes us.

use crate::types::MailError;
use serde::{Deserialize, Serialize};

pub mod app_lock;
pub mod compose;
pub mod html_view;
pub mod image_load;

/// Effects the app performs (network calls, wakeups) and feeds results back as events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    Connect,
    Authenticate,
    SyncNext,
    StartIdle,
    WakeAt(f64),
    Send { attempt: u32 },
    ShowUndo { until: f64 },
    Notify(String),
}

pub mod account_sync {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum State {
        Disconnected,
        Connecting { attempt: u32 },
        Authenticating { attempt: u32 },
        Syncing { done: u32, total: u32 },
        Idle,
        Backoff { attempt: u32, due: f64 },
        AuthFailed,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Event {
        Start,
        Connected,
        Authenticated,
        FolderSynced { total: u32 },
        Failed(MailError),
        Timer(f64),
        IdleWoke,
    }

    /// Backoff: 1, 2, 4 … capped at 300 s.
    pub fn backoff_secs(attempt: u32) -> f64 { (1u64 << attempt.min(9)).min(300) as f64 }

    pub fn step(s: &mut State, ev: Event, now: f64) -> Vec<Effect> {
        use State::*;
        match (std::mem::replace(s, Disconnected), ev) {
            (Disconnected, Event::Start) => { *s = Connecting { attempt: 0 }; vec![Effect::Connect] }
            (Connecting { attempt }, Event::Connected) => { *s = Authenticating { attempt }; vec![Effect::Authenticate] }
            (Authenticating { .. }, Event::Authenticated) => { *s = Syncing { done: 0, total: 0 }; vec![Effect::SyncNext] }
            (Syncing { done, .. }, Event::FolderSynced { total }) => {
                let done = done + 1;
                if done >= total { *s = Idle; vec![Effect::StartIdle] } else { *s = Syncing { done, total }; vec![Effect::SyncNext] }
            }
            (Idle, Event::IdleWoke) => { *s = Syncing { done: 0, total: 0 }; vec![Effect::SyncNext] }
            (_, Event::Failed(MailError::Auth)) => { *s = AuthFailed; vec![Effect::Notify("Sign-in failed".into())] }
            (prev, Event::Failed(_)) => {
                let attempt = match prev { Connecting { attempt } | Authenticating { attempt } | Backoff { attempt, .. } => attempt + 1, _ => 1 };
                let due = now + backoff_secs(attempt);
                *s = Backoff { attempt, due };
                vec![Effect::WakeAt(due)]
            }
            (Backoff { attempt, due }, Event::Timer(t)) if t >= due => { *s = Connecting { attempt }; vec![Effect::Connect] }
            (prev, _) => { *s = prev; vec![] } // events that don't apply change nothing
        }
    }
}

pub mod outbox {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum State {
        Queued { undo_until: f64 },
        Sending { attempt: u32 },
        Sent,
        Failed { error: MailError, attempt: u32 },
        Cancelled,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Event { Undo, Timer(f64), SendOk, SendErr(MailError), Retry }

    pub fn queue(now: f64, undo_secs: f64) -> (State, Vec<Effect>) {
        let until = now + undo_secs;
        (State::Queued { undo_until: until }, vec![Effect::ShowUndo { until }, Effect::WakeAt(until)])
    }

    pub fn step(s: &mut State, ev: Event) -> Vec<Effect> {
        use State::*;
        match (s.clone(), ev) {
            (Queued { .. }, Event::Undo) => { *s = Cancelled; vec![] }
            (Queued { undo_until }, Event::Timer(t)) if t >= undo_until => { *s = Sending { attempt: 1 }; vec![Effect::Send { attempt: 1 }] }
            (Sending { .. }, Event::SendOk) => { *s = Sent; vec![Effect::Notify("Sent".into())] }
            (Sending { attempt }, Event::SendErr(error)) => { *s = Failed { error, attempt }; vec![Effect::Notify("Send failed".into())] }
            (Failed { attempt, .. }, Event::Retry) => { *s = Sending { attempt: attempt + 1 }; vec![Effect::Send { attempt: attempt + 1 }] }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests;

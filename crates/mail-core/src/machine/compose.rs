//! `ComposeSession`: one open draft. Autosave is a timer event, so a quiet draft saves nothing.

use serde::{Deserialize, Serialize};

/// Seconds of quiet after the last edit before a draft is saved.
pub const AUTOSAVE_SECS: f64 = 3.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum State {
    /// `dirty_since` is the time of the first unsaved edit.
    Editing { dirty_since: Option<f64> },
    /// A save is in flight; `dirty` records an edit that arrived meanwhile.
    SavingDraft { dirty: bool },
    Sending,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event { Edit, Timer(f64), DraftSaved, SaveFailed, Send, Discard }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect { WakeAt(f64), AppendDraft, QueueSend, DeleteDraft, Notify(String) }

pub fn open() -> State { State::Editing { dirty_since: None } }

pub fn step(s: &mut State, ev: Event, now: f64) -> Vec<Effect> {
    use State::*;
    match (s.clone(), ev) {
        (Editing { dirty_since: None }, Event::Edit) => {
            *s = Editing { dirty_since: Some(now) };
            vec![Effect::WakeAt(now + AUTOSAVE_SECS)]
        }
        (Editing { dirty_since: Some(since) }, Event::Timer(t)) if t >= since + AUTOSAVE_SECS => {
            *s = SavingDraft { dirty: false };
            vec![Effect::AppendDraft]
        }
        (SavingDraft { .. }, Event::Edit) => { *s = SavingDraft { dirty: true }; vec![] }
        (SavingDraft { dirty }, Event::DraftSaved) => {
            *s = Editing { dirty_since: dirty.then_some(now) };
            if dirty { vec![Effect::WakeAt(now + AUTOSAVE_SECS)] } else { vec![] }
        }
        (SavingDraft { .. }, Event::SaveFailed) => {
            // Keep the text and try again on the next quiet period.
            *s = Editing { dirty_since: Some(now) };
            vec![Effect::WakeAt(now + AUTOSAVE_SECS), Effect::Notify("Draft not saved".into())]
        }
        (Editing { .. } | SavingDraft { .. }, Event::Send) => { *s = Sending; vec![Effect::QueueSend] }
        (Editing { .. } | SavingDraft { .. }, Event::Discard) => { *s = Closed; vec![Effect::DeleteDraft] }
        (Sending, Event::DraftSaved) => { *s = Closed; vec![] }
        _ => vec![],
    }
}

//! `AppLock`: the biometric app lock (Touch ID, Windows Hello, a passkey on the web). While
//! locked the UI draws only the lock screen; the platform layer shows the OS prompt and reports
//! the result. Idle time is a timer event, like every other timer here.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum State {
    Unlocked { last_activity: f64 },
    /// Locked, with no prompt on screen.
    Locked,
    /// The OS prompt is showing.
    Prompting,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// A key press or pointer action.
    Activity(f64),
    Timer(f64),
    /// The user asked to lock, or the machine went to sleep.
    LockNow,
    /// The user asked to unlock (any key on the lock screen).
    Unlock,
    Prompted { ok: bool },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    WakeAt(f64),
    /// Hide message content now, before anything else is drawn.
    Conceal,
    /// Show the OS biometric or password prompt.
    Prompt,
    Reveal,
}

/// `idle_secs` of 0 means the app never locks on its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Policy { pub idle_secs: f64, pub lock_on_launch: bool }

pub fn launch(policy: Policy, now: f64) -> (State, Vec<Effect>) {
    if policy.lock_on_launch { return (State::Prompting, vec![Effect::Conceal, Effect::Prompt]); }
    (State::Unlocked { last_activity: now }, idle_timer(policy, now))
}

fn idle_timer(policy: Policy, from: f64) -> Vec<Effect> {
    if policy.idle_secs > 0.0 { vec![Effect::WakeAt(from + policy.idle_secs)] } else { vec![] }
}

pub fn step(s: &mut State, ev: Event, policy: Policy) -> Vec<Effect> {
    use State::*;
    match (s.clone(), ev) {
        (Unlocked { .. }, Event::Activity(t)) => { *s = Unlocked { last_activity: t }; vec![] }
        (Unlocked { last_activity }, Event::Timer(t)) if policy.idle_secs > 0.0 => {
            let due = last_activity + policy.idle_secs;
            if t >= due { *s = Locked; vec![Effect::Conceal] } else { vec![Effect::WakeAt(due)] }
        }
        (Unlocked { .. }, Event::LockNow) => { *s = Locked; vec![Effect::Conceal] }
        (Locked, Event::Unlock | Event::Activity(_)) => { *s = Prompting; vec![Effect::Prompt] }
        (Prompting, Event::Prompted { ok: true }) => vec![Effect::Reveal],
        (Prompting, Event::Prompted { ok: false }) => { *s = Locked; vec![] }
        _ => vec![],
    }
}

/// Finish a successful prompt. Separate from `step` because it needs the time of the unlock.
pub fn unlocked(s: &mut State, policy: Policy, now: f64) -> Vec<Effect> {
    if *s != State::Prompting { return vec![]; }
    *s = State::Unlocked { last_activity: now };
    idle_timer(policy, now)
}

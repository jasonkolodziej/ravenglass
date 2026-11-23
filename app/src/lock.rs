//! The app lock (plan §8: v1), driving `mail_core::machine::app_lock` from the config's
//! `privacy.lock_after_minutes` and `lock_on_launch`. While it is concealed the view draws only
//! the wallpaper and the lock line (`view/lock.rs`); nothing with message content is built.
//!
//! How the user proves who they are is the platform's business (`platform::authenticate`). The
//! portable default is the password of the first password account, typed on the lock screen and
//! compared through the `SecretStore`. With no such account there is nothing to compare, so the
//! lock is a privacy curtain that any key lifts.

use crate::accounts_ui::Secret;
use crate::keys::{K, KeyPress};
use crate::platform::{self, Unlock};
use mail_core::machine::app_lock::{self, Effect, Event, Policy, State};

#[derive(Debug)]
pub struct Lock {
    pub state: State,
    pub policy: Policy,
    /// The password being typed on the lock screen.
    pub input: Secret,
    /// The last password was wrong.
    pub refused: bool,
    /// A password is being compared off the UI thread.
    pub checking: bool,
    /// When the idle timer next needs a look, on the app clock.
    due: Option<f64>,
}

/// What a key on the lock screen asks of the app.
#[derive(Debug)]
pub enum Asked {
    Nothing,
    /// Compare this password with the stored one, off the UI thread, then call [`Lock::verified`].
    Verify(Secret),
}

impl Default for Lock {
    fn default() -> Self { Lock::launch(Policy { idle_secs: 0.0, lock_on_launch: false }, 0.0, false) }
}

impl Lock {
    /// `can_verify`: there is a password account to check a typed password against.
    pub fn launch(policy: Policy, now: f64, can_verify: bool) -> Lock {
        let (state, effects) = app_lock::launch(policy, now);
        let mut lock = Lock { state, policy, input: Secret::new(), refused: false, checking: false, due: None };
        lock.note(&effects);
        if effects.contains(&Effect::Prompt) { lock.prompt(can_verify, now); }
        lock
    }

    /// A new policy from the config, keeping the current state.
    pub fn set_policy(&mut self, policy: Policy, now: f64) {
        self.policy = policy;
        self.due = match (&self.state, policy.idle_secs > 0.0) {
            (State::Unlocked { last_activity }, true) => Some((last_activity + policy.idle_secs).max(now)),
            _ => None,
        };
    }

    /// Message content must not be on screen.
    pub fn concealed(&self) -> bool { !matches!(self.state, State::Unlocked { .. }) }

    /// The lock screen is asking for a password.
    pub fn prompting(&self) -> bool { self.state == State::Prompting }

    /// The next time [`Lock::tick`] has something to do.
    pub fn due(&self) -> Option<f64> { self.due }

    fn note(&mut self, effects: &[Effect]) {
        for effect in effects {
            if let Effect::WakeAt(t) = effect { self.due = Some(*t); }
        }
    }

    /// A key or pointer action while unlocked.
    pub fn activity(&mut self, now: f64) {
        if !self.concealed() { app_lock::step(&mut self.state, Event::Activity(now), self.policy); }
    }

    /// The idle timer. True when the app just locked.
    pub fn tick(&mut self, now: f64) -> bool {
        if self.due.is_none_or(|due| now < due) { return false; }
        self.due = None;
        let effects = app_lock::step(&mut self.state, Event::Timer(now), self.policy);
        self.note(&effects);
        effects.contains(&Effect::Conceal)
    }

    pub fn lock_now(&mut self) {
        app_lock::step(&mut self.state, Event::LockNow, self.policy);
        self.due = None;
        self.input.wipe();
        self.refused = false;
    }

    /// The machine asked for the prompt: the OS's if there is one, else the password line.
    fn prompt(&mut self, can_verify: bool, now: f64) {
        match platform::authenticate() {
            Unlock::Os(ok) => self.verified(ok, now, can_verify),
            Unlock::Password if !can_verify => self.verified(true, now, can_verify),
            Unlock::Password => {}
        }
    }

    /// A key while concealed. Nothing typed here reaches the rest of the app.
    pub fn key(&mut self, key: KeyPress, can_verify: bool, now: f64) -> Asked {
        match self.state {
            State::Unlocked { .. } => {}
            State::Locked => {
                let effects = app_lock::step(&mut self.state, Event::Unlock, self.policy);
                if effects.contains(&Effect::Prompt) { self.prompt(can_verify, now); }
            }
            State::Prompting if self.checking => {}
            State::Prompting => match key.key {
                K::Esc => {
                    app_lock::step(&mut self.state, Event::Prompted { ok: false }, self.policy);
                    self.input.wipe();
                    self.refused = false;
                }
                K::Backspace => self.input.pop(),
                K::Enter if !self.input.is_empty() => {
                    self.checking = true;
                    self.refused = false;
                    return Asked::Verify(std::mem::take(&mut self.input));
                }
                _ => if let Some(c) = key.typed() { self.input.push(c); },
            },
        }
        Asked::Nothing
    }

    /// The prompt answered. A wrong password asks again; the right one reveals.
    pub fn verified(&mut self, ok: bool, now: f64, can_verify: bool) {
        self.checking = false;
        self.input.wipe();
        let effects = app_lock::step(&mut self.state, Event::Prompted { ok }, self.policy);
        if effects.contains(&Effect::Reveal) {
            self.refused = false;
            let effects = app_lock::unlocked(&mut self.state, self.policy, now);
            self.note(&effects);
        } else if !ok && self.state == State::Locked && can_verify {
            self.refused = true;
            app_lock::step(&mut self.state, Event::Unlock, self.policy);
        }
    }
}

//! The app lock (plan §8: v1), driving `mail_core::machine::app_lock` from the config's
//! `privacy.lock_after_minutes` and `lock_on_launch`. While it is concealed the view draws only
//! the wallpaper and the lock line (`view/lock.rs`); nothing with message content is built.
//!
//! How the user proves who they are is the platform's business (`platform::authenticate`). The
//! portable default is something typed on the lock screen and checked off the UI thread: the app
//! passcode when one is set (`:lock passcode`), else the password of the first password account.
//!
//! The lock never closes without a way to open it: with nothing to check against it stays
//! unarmed (no lock on launch, no idle lock, and `:lock` asks for a passcode to be set first).
//! Wrong answers are throttled: after three, each attempt waits longer, up to fifteen minutes.

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
    /// There is something to check a typed answer against, so the lock may close.
    armed: bool,
    /// Wrong answers in a row.
    failures: u32,
    /// No answer is taken before this time on the app clock.
    pub retry_at: Option<f64>,
}

/// Wrong answers allowed before attempts are spaced out.
const FREE_ATTEMPTS: u32 = 3;
/// The longest wait between attempts, in seconds.
const MAX_WAIT: f64 = 900.0;

/// The wait after `failures` wrong answers: none for the first few, then 5 s doubling.
pub fn wait_after(failures: u32) -> f64 {
    if failures < FREE_ATTEMPTS { return 0.0; }
    (5.0 * f64::from(1u32 << (failures - FREE_ATTEMPTS).min(10))).min(MAX_WAIT)
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

/// What the lock does with no way to open it: nothing on its own.
const UNARMED: Policy = Policy { idle_secs: 0.0, lock_on_launch: false };

impl Lock {
    /// `can_verify`: there is a password account to check a typed password against.
    pub fn launch(policy: Policy, now: f64, can_verify: bool) -> Lock {
        let effective = if can_verify { policy } else { UNARMED };
        let (state, effects) = app_lock::launch(effective, now);
        let mut lock = Lock { state, policy: effective, input: Secret::new(), refused: false, checking: false, due: None, armed: can_verify, failures: 0, retry_at: None };
        lock.note(&effects);
        if effects.contains(&Effect::Prompt) { lock.prompt(can_verify, now); }
        lock
    }

    /// A new policy from the config, or a new answer to whether anything can open the lock,
    /// keeping the current state.
    pub fn set_policy(&mut self, policy: Policy, now: f64, can_verify: bool) {
        self.armed = can_verify;
        let policy = if can_verify { policy } else { UNARMED };
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

    /// The lock may close: something can check an answer.
    pub fn armed(&self) -> bool { self.armed }

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

    /// Lock now. False, and nothing changes, when nothing could open it again.
    pub fn lock_now(&mut self) -> bool {
        if !self.armed { return false; }
        app_lock::step(&mut self.state, Event::LockNow, self.policy);
        self.due = None;
        self.input.wipe();
        self.refused = false;
        true
    }

    /// Seconds until another answer is taken, if it must wait.
    pub fn waiting(&self, now: f64) -> Option<f64> { self.retry_at.filter(|at| *at > now).map(|at| at - now) }

    /// The machine asked for the prompt: the OS's if there is one, else the password line.
    fn prompt(&mut self, can_verify: bool, now: f64) {
        match platform::authenticate() {
            Unlock::Os(ok) => self.verified(ok, now, can_verify),
            // Nothing to check against: stay locked rather than open for anyone. An armed lock
            // cannot get here; this is the backstop.
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
                // Too many wrong answers: the typed text waits until the throttle passes.
                K::Enter if self.waiting(now).is_some() => {}
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
            self.failures = 0;
            self.retry_at = None;
            let effects = app_lock::unlocked(&mut self.state, self.policy, now);
            self.note(&effects);
        } else if !ok && self.state == State::Locked && can_verify {
            self.refused = true;
            self.failures += 1;
            let wait = wait_after(self.failures);
            self.retry_at = (wait > 0.0).then_some(now + wait);
            app_lock::step(&mut self.state, Event::Unlock, self.policy);
        }
    }
}

//! Setting, changing and removing the app-lock passcode (`:lock passcode`, `:lock passcode off`):
//! a small state machine over masked input. Changing or removing an existing passcode asks for
//! it first. The screen it draws is full-window, like the lock, so nothing typed sits next to mail.

use crate::accounts_ui::Secret;
use crate::keys::{K, KeyPress};
use mail_auth::passcode::MIN_LEN;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goal { Set, Clear }

#[derive(Debug)]
pub enum Step {
    /// The current passcode, before changing or removing it.
    Current { input: Secret },
    /// A current passcode is being checked off the UI thread.
    Checking,
    New { input: Secret },
    Repeat { first: Secret, input: Secret },
    /// The new passcode is being hashed and stored.
    Saving,
}

#[derive(Debug)]
pub struct Setup {
    pub goal: Goal,
    pub step: Step,
    /// One line about the last answer (too short, did not match, wrong passcode).
    pub note: Option<&'static str>,
}

/// What the setup asks of the app.
#[derive(Debug)]
pub enum Asked {
    Nothing,
    /// Check this against the stored passcode, then call [`Setup::checked`].
    Check(Secret),
    /// Hash and store this as the passcode, then close the setup.
    Store(Secret),
    /// Remove the stored passcode, then close the setup.
    Remove,
    /// The person left the setup.
    Cancelled,
}

impl Setup {
    /// `exists`: a passcode is stored now.
    pub fn start(goal: Goal, exists: bool) -> Setup {
        let step = if exists { Step::Current { input: Secret::new() } } else { Step::New { input: Secret::new() } };
        Setup { goal, step, note: None }
    }

    pub fn key(&mut self, key: KeyPress) -> Asked {
        if key.key == K::Esc {
            self.wipe();
            return Asked::Cancelled;
        }
        let input = match &mut self.step {
            Step::Current { input } | Step::New { input } | Step::Repeat { input, .. } => input,
            Step::Checking | Step::Saving => return Asked::Nothing,
        };
        match key.key {
            K::Backspace => input.pop(),
            K::Enter => return self.enter(),
            _ => if let Some(c) = key.typed() { input.push(c); },
        }
        Asked::Nothing
    }

    fn enter(&mut self) -> Asked {
        self.note = None;
        match std::mem::replace(&mut self.step, Step::Checking) {
            Step::Current { input } if input.is_empty() => { self.step = Step::Current { input }; Asked::Nothing }
            Step::Current { input } => Asked::Check(input),
            Step::New { input } if input.len() < MIN_LEN => {
                let mut input = input;
                input.wipe();
                self.note = Some("too short: at least 6 characters");
                self.step = Step::New { input };
                Asked::Nothing
            }
            Step::New { input } => { self.step = Step::Repeat { first: input, input: Secret::new() }; Asked::Nothing }
            Step::Repeat { mut first, mut input } if input.expose() != first.expose() => {
                first.wipe();
                input.wipe();
                self.note = Some("they did not match: type a new one again");
                self.step = Step::New { input: Secret::new() };
                Asked::Nothing
            }
            Step::Repeat { first, mut input } => {
                input.wipe();
                self.step = Step::Saving;
                Asked::Store(first)
            }
            other => { self.step = other; Asked::Nothing }
        }
    }

    /// The current passcode was checked.
    pub fn checked(&mut self, ok: bool) -> Asked {
        if !matches!(self.step, Step::Checking) { return Asked::Nothing; }
        if !ok {
            self.note = Some("that was not the passcode");
            self.step = Step::Current { input: Secret::new() };
            return Asked::Nothing;
        }
        match self.goal {
            Goal::Set => { self.step = Step::New { input: Secret::new() }; Asked::Nothing }
            Goal::Clear => { self.step = Step::Saving; Asked::Remove }
        }
    }

    /// What the screen says: the question and one `•` per character typed.
    pub fn caption(&self) -> String {
        match &self.step {
            Step::Current { input } => format!("Current passcode  {}_", input.masked()),
            Step::New { input } => format!("New passcode  {}_", input.masked()),
            Step::Repeat { input, .. } => format!("Type it again  {}_", input.masked()),
            Step::Checking => "Checking…".into(),
            Step::Saving => "Saving…".into(),
        }
    }

    pub fn hint(&self) -> String {
        let base = match (&self.step, self.goal) {
            (Step::Current { .. }, Goal::Clear) => "enter remove the passcode    esc cancel",
            (Step::Current { .. }, Goal::Set) => "enter continue    esc cancel",
            (Step::New { .. }, _) => "at least 6 characters    enter continue    esc cancel",
            (Step::Repeat { .. }, _) => "enter save    esc cancel",
            _ => "",
        };
        match self.note { Some(note) => format!("{note}    {base}"), None => base.to_owned() }
    }

    fn wipe(&mut self) {
        match &mut self.step {
            Step::Current { input } | Step::New { input } => input.wipe(),
            Step::Repeat { first, input } => { first.wipe(); input.wipe(); }
            Step::Checking | Step::Saving => {}
        }
    }
}

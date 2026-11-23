//! Signed-in sessions (users mode). A session token is 32 random bytes handed to the browser in
//! an `HttpOnly` cookie; the Relay keeps only its SHA-256, so a memory dump does not yield
//! usable cookies. Each session also has a CSRF token the browser echoes on mutating requests.
//!
//! A session is `Live` until it has been idle for [`IDLE`] or exists for [`ABSOLUTE`], then
//! `Expired`; [`Sessions::touch`] is the only way to use one, and it applies both limits.
//! Sessions are in memory only: restarting the Relay signs everyone out.

use crate::crypto::{digest, random_token};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Seconds since the Unix epoch. Injected so tests can move time.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()))
}

/// Idle timeout.
pub const IDLE: u64 = 30 * 60;
/// Absolute lifetime.
pub const ABSOLUTE: u64 = 12 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session { pub user: i64, pub csrf: String, pub created: u64, pub last: u64 }

impl Session {
    pub fn expired(&self, now: u64) -> bool { now >= self.created + ABSOLUTE || now >= self.last + IDLE }
}

#[derive(Default)]
pub struct Sessions { live: Mutex<HashMap<String, Session>> }

impl Sessions {
    /// A new session for `user`: returns the cookie token (never stored) and the session.
    pub fn create(&self, user: i64, now: u64) -> (String, Session) {
        let token = random_token(32);
        let session = Session { user, csrf: random_token(32), created: now, last: now };
        let mut live = self.live.lock().expect("sessions lock");
        live.retain(|_, s| !s.expired(now));
        live.insert(digest(&token), session.clone());
        (token, session)
    }

    /// Use a session: `None` if it is unknown or expired (and then it is gone).
    pub fn touch(&self, token: &str, now: u64) -> Option<Session> {
        let key = digest(token);
        let mut live = self.live.lock().expect("sessions lock");
        let session = live.get_mut(&key)?;
        if session.expired(now) {
            live.remove(&key);
            return None;
        }
        session.last = now;
        Some(session.clone())
    }

    /// Whether a session is still live, without counting this as use.
    pub fn alive(&self, token: &str, now: u64) -> bool { self.live.lock().expect("sessions lock").get(&digest(token)).is_some_and(|s| !s.expired(now)) }

    pub fn end(&self, token: &str) { self.live.lock().expect("sessions lock").remove(&digest(token)); }

    /// End every session of a user (they were removed).
    pub fn end_user(&self, user: i64) { self.live.lock().expect("sessions lock").retain(|_, s| s.user != user); }

    /// Whether the user has a session that has not expired.
    pub fn has_live(&self, user: i64, now: u64) -> bool { self.live.lock().expect("sessions lock").values().any(|s| s.user == user && !s.expired(now)) }
}

#[cfg(test)]
mod tests;

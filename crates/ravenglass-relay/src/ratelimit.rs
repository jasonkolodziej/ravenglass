//! Sign-in rate limiting with exponential backoff, keyed by client address and by username.
//!
//! Each key may fail `free` times; after that every further attempt must wait 2^(n-free)
//! seconds after the last failure, capped at [`MAX_DELAY`]. A key that has not failed for
//! [`FORGET`] starts over, and a successful sign-in clears its username key.

use std::collections::HashMap;
use std::sync::Mutex;

pub const MAX_DELAY: u64 = 15 * 60;
pub const FORGET: u64 = 60 * 60;
/// More keys than this and old ones are swept on the next failure.
const SWEEP_AT: usize = 10_000;

#[derive(Clone, Copy)]
struct Entry { failures: u32, last: u64 }

pub struct Limiter { free: u32, entries: Mutex<HashMap<String, Entry>> }

impl Limiter {
    pub fn new(free: u32) -> Self { Self { free, entries: Mutex::new(HashMap::new()) } }

    fn delay(&self, entry: Entry) -> u64 {
        if entry.failures < self.free { return 0; }
        let over = (entry.failures - self.free).min(20);
        (1u64 << over).min(MAX_DELAY)
    }

    /// `Err(seconds)` while `key` must wait.
    pub fn check(&self, key: &str, now: u64) -> Result<(), u64> {
        let entries = self.entries.lock().expect("limiter lock");
        let Some(entry) = entries.get(key).copied() else { return Ok(()) };
        if now >= entry.last + FORGET { return Ok(()); }
        let ready = entry.last + self.delay(entry);
        if now >= ready { Ok(()) } else { Err(ready - now) }
    }

    pub fn fail(&self, key: &str, now: u64) {
        let mut entries = self.entries.lock().expect("limiter lock");
        if entries.len() >= SWEEP_AT { entries.retain(|_, e| now < e.last + FORGET); }
        let entry = entries.entry(key.to_owned()).or_insert(Entry { failures: 0, last: now });
        if now >= entry.last + FORGET { entry.failures = 0; }
        entry.failures = entry.failures.saturating_add(1);
        entry.last = now;
    }

    pub fn succeed(&self, key: &str) { self.entries.lock().expect("limiter lock").remove(key); }
}

#[cfg(test)]
mod tests;

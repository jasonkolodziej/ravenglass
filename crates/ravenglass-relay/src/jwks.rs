//! The identity provider's key set, cached in memory: when to use it, when to fetch it again.
//!
//! A pure state machine on an injected clock; the server does the fetching. The key set lives
//! for the response's `Cache-Control: max-age`, held between [`MIN_LIFETIME`] and
//! [`MAX_LIFETIME`]. A token naming a key the set does not have may trigger one early refetch
//! (providers rotate keys), but fetches, successful or not, are at least [`MIN_REFETCH`] apart,
//! so a stream of bad tokens cannot make the Relay hammer the provider.

use crate::jose::KeySet;
use std::sync::Arc;

/// Shortest time a fetched key set is kept, whatever the provider says.
pub const MIN_LIFETIME: u64 = 5 * 60;
/// Longest time a fetched key set is kept, whatever the provider says.
pub const MAX_LIFETIME: u64 = 24 * 60 * 60;
/// How long a key set is kept when the provider gives no `max-age`.
pub const DEFAULT_LIFETIME: u64 = 60 * 60;
/// Least time between two fetches.
pub const MIN_REFETCH: u64 = 60;

/// How long to keep a key set served with this `Cache-Control` header.
pub fn lifetime(cache_control: Option<&str>) -> u64 {
    let Some(header) = cache_control else { return DEFAULT_LIFETIME };
    let mut max_age = None;
    for directive in header.split(',').map(str::trim) {
        let (name, value) = directive.split_once('=').map_or((directive, ""), |(n, v)| (n.trim(), v.trim().trim_matches('"')));
        match name.to_ascii_lowercase().as_str() {
            "no-store" | "no-cache" => return MIN_LIFETIME,
            "max-age" => max_age = value.parse::<u64>().ok(),
            _ => {}
        }
    }
    max_age.map_or(DEFAULT_LIFETIME, |age| age.clamp(MIN_LIFETIME, MAX_LIFETIME))
}

#[derive(Default)]
pub struct KeyCache {
    keys: Option<Arc<KeySet>>,
    /// When `keys` stops being current.
    expires: u64,
    /// When the last fetch started, whatever came of it.
    last_fetch: Option<u64>,
}

/// What to do before verifying a token.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Use the cached key set.
    Use,
    /// Fetch the key set (then call [`KeyCache::fetched`] or [`KeyCache::failed`]).
    Fetch,
    /// There is no key set and fetching is held back: refuse.
    Refuse,
}

impl KeyCache {
    /// The key set to verify with, current or stale.
    pub fn keys(&self) -> Option<Arc<KeySet>> { self.keys.clone() }

    fn may_fetch(&self, now: u64) -> bool { self.last_fetch.is_none_or(|last| now >= last + MIN_REFETCH) }

    /// Before verifying: fetch when there is no current key set and fetching is allowed. A
    /// stale set is used while a refetch is held back (the last one failed moments ago).
    pub fn plan(&self, now: u64) -> Plan {
        let current = self.keys.is_some() && now < self.expires;
        if current { return Plan::Use; }
        if self.may_fetch(now) { return Plan::Fetch; }
        if self.keys.is_some() { Plan::Use } else { Plan::Refuse }
    }

    /// After a token named a key the set does not have: refetch, unless a fetch was too recent.
    pub fn plan_unknown_key(&self, now: u64) -> Plan { if self.may_fetch(now) { Plan::Fetch } else { Plan::Refuse } }

    /// A fetch starting at `now` succeeded.
    pub fn fetched(&mut self, keys: KeySet, lifetime: u64, now: u64) {
        self.keys = Some(Arc::new(keys));
        self.expires = now + lifetime;
        self.last_fetch = Some(now);
    }

    /// A fetch starting at `now` failed. Any key set already held is kept.
    pub fn failed(&mut self, now: u64) { self.last_fetch = Some(now); }
}

#[cfg(test)]
mod tests;

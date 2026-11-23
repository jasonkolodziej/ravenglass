//! `ImageLoad`: the remote-image policy of one open message. Nothing is fetched until the user
//! allows it; an allow for the sender is remembered by the caller (`image_policy` in the store).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum State {
    /// `remote` is how many remote images the sanitizer found and held back.
    Blocked { remote: u32 },
    Loading { pending: u32, failed: u32 },
    Loaded,
    /// Some images failed; the rest are shown.
    Failed { failed: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope { Once, Sender }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event { Allow(Scope), Fetched { ok: bool } }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect { RememberSender, FetchAll, Reraster }

/// The state a message opens in: allowed senders skip straight to loading.
pub fn open(remote: u32, sender_allowed: bool) -> (State, Vec<Effect>) {
    match (remote, sender_allowed) {
        (0, _) => (State::Loaded, vec![]),
        (n, true) => (State::Loading { pending: n, failed: 0 }, vec![Effect::FetchAll]),
        (n, false) => (State::Blocked { remote: n }, vec![]),
    }
}

pub fn step(s: &mut State, ev: Event) -> Vec<Effect> {
    use State::*;
    match (s.clone(), ev) {
        (Blocked { remote }, Event::Allow(scope)) => {
            *s = Loading { pending: remote, failed: 0 };
            let mut fx = vec![Effect::FetchAll];
            if scope == Scope::Sender { fx.insert(0, Effect::RememberSender); }
            fx
        }
        (Loading { pending, failed }, Event::Fetched { ok }) => {
            let (pending, failed) = (pending.saturating_sub(1), failed + u32::from(!ok));
            *s = match (pending, failed) { (0, 0) => Loaded, (0, f) => Failed { failed: f }, (p, f) => Loading { pending: p, failed: f } };
            vec![Effect::Reraster]
        }
        _ => vec![],
    }
}

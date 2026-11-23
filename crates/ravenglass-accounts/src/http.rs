//! Blocking HTTPS helpers for native builds: the token-endpoint POST and the autoconfig fetch.

use crate::FormPost;
use std::sync::Arc;
use std::time::Duration;

fn agent(status_as_error: bool) -> ureq::Agent {
    let config = ureq::Agent::config_builder().https_only(true).http_status_as_error(status_as_error).max_redirects(3).timeout_global(Some(Duration::from_secs(20))).user_agent("Ravenglass").build();
    ureq::Agent::new_with_config(config)
}

/// A [`FormPost`] over HTTPS. A non-2xx answer still returns its body: OAuth errors are JSON.
pub fn form_post() -> FormPost {
    let agent = agent(false);
    Arc::new(move |url: &str, body: &str| {
        let mut response = agent.post(url).header("content-type", "application/x-www-form-urlencoded").header("accept", "application/json").send(body).map_err(|e| e.to_string())?;
        response.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| e.to_string())
    })
}

/// Fetch an autoconfig document for `mail_auth::autoconfig::discover`; `None` on any failure.
pub fn fetch_document(url: &str) -> Option<String> {
    let mut response = agent(true).get(url).call().ok()?;
    response.body_mut().with_config().limit(1 << 20).read_to_string().ok()
}

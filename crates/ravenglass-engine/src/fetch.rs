//! Fetching remote images the user allowed (architecture §7 "Images"), for native builds. The
//! web build asks the Relay's image proxy instead.

use crate::ImageFetcher;
use std::sync::Arc;
use std::time::Duration;

/// A fetcher that sends no cookies and no referrer, follows few redirects, gives up after ten
/// seconds, and stops reading at the per-image cap. It blocks the calling (engine) thread.
pub fn https_images() -> ImageFetcher {
    let config = ureq::Agent::config_builder().http_status_as_error(true).max_redirects(3).timeout_global(Some(Duration::from_secs(10))).user_agent("Ravenglass").build();
    let agent = ureq::Agent::new_with_config(config);
    Arc::new(move |url: &str| {
        let mut response = agent.get(url).call().ok()?;
        response.body_mut().with_config().limit(mail_blitz::MAX_IMAGE_BYTES as u64).read_to_vec().ok()
    })
}

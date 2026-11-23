//! `mailto:` links the system opens with the app become drafts.

use crate::state::AppState;
use mail_render::mailto::encode;

const LINK: &str = "mailto:odin@asgard.example?cc=muninn@ravens.example&subject=Ferry%20times&body=When%20does%20it%20leave%3F";

#[test]
fn a_mailto_link_opens_a_filled_in_draft_that_the_engine_keeps() {
    let mut s = AppState::demo_inline();
    s.open_system_link(LINK);
    let c = s.compose.current().expect("a draft opened");
    assert_eq!(c.draft.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["odin@asgard.example"]);
    assert_eq!((c.draft.cc.len(), c.draft.subject.as_str(), c.draft.body.as_str()), (1, "Ferry times", "When does it leave?"));
    assert!(c.to.text.contains("odin@asgard.example") && c.subject.text.contains("Ferry times"), "the fields show it");
    assert!(c.outbox.is_none(), "a link never sends anything");
}

#[test]
fn a_wrapped_link_from_another_launch_works_the_same() {
    let mut s = AppState::demo_inline();
    s.open_system_link(&format!("ravenglass://mailto?u={}", encode(LINK)));
    assert_eq!(s.compose.current().map(|c| c.draft.subject.as_str()), Some("Ferry times"));
}

#[test]
fn a_link_before_the_engine_starts_waits_for_it() {
    let mut s = AppState::windowed();
    s.open_system_link(LINK);
    assert!(s.compose.current().is_none() && s.compose.waiting_link.is_some(), "nothing to send the draft to yet");
    // What `poll` does once the first frame is out, with the inline engine standing in.
    s.link = crate::link::Link::inline(mail_mock::NOW as f64).expect("the demo engine");
    s.open_accounts();
    s.open_waiting_link();
    assert_eq!(s.compose.current().map(|c| c.draft.subject.as_str()), Some("Ferry times"));
}

#[test]
fn other_links_are_ignored() {
    let mut s = AppState::demo_inline();
    for link in ["https://example.com", "ravenglass://other", "file:///etc/passwd", "mailto:not-an-address", "javascript:alert(1)"] {
        s.open_system_link(link);
    }
    assert!(s.compose.drafts.iter().all(|c| c.draft.to.is_empty() && c.draft.subject.is_empty()), "nothing was filled in");
}

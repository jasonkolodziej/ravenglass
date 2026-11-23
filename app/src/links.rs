//! Links in the HTML reader (plan §3.4 "Pointer"): the pointer's place goes to the engine, which
//! answers with the link under it; the status line shows where it leads, and a click opens it
//! through `platform::open_url`. A link whose text names another host than its target asks first.

use crate::fold::HTML_VIEW;
use crate::keys::{K, KeyPress};
use crate::state::AppState;
use ravenglass_engine::{Command, Link};

#[derive(Default)]
pub struct Links {
    /// The link under the pointer.
    pub hover: Option<Link>,
    /// A link waiting for `y` or `n` in the status line.
    pub confirm: Option<Link>,
    /// The last point asked about, in whole CSS pixels.
    asked: Option<(i32, i32)>,
}

/// The host of a web address, for the question in the status line.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.rsplit('@').next().unwrap_or(host)
}

impl AppState {
    /// The pointer moved over the HTML card, to this point of the document in CSS pixels.
    pub fn html_pointer(&mut self, x: f32, y: f32) {
        let point = (x.floor() as i32, y.floor() as i32);
        if self.links.asked == Some(point) { return; }
        self.links.asked = Some(point);
        self.dispatch(Command::HtmlPointer { view: HTML_VIEW, x, y });
    }

    /// The pointer left the card.
    pub fn html_pointer_left(&mut self) {
        self.links.asked = None;
        self.links.hover = None;
    }

    /// A click on the card: open the link under the pointer, asking first when its text reads
    /// like a different site (a phishing guard).
    pub fn html_click(&mut self) {
        let Some(link) = self.links.hover.clone() else { return };
        if link.text_names_another_host() {
            self.links.confirm = Some(link);
        } else {
            self.open_link(&link.href);
        }
    }

    /// While a link waits for its answer every key is the answer: `y` opens, anything else
    /// does not. True when a question was open.
    pub fn link_confirm_key(&mut self, press: KeyPress) -> bool {
        let Some(link) = self.links.confirm.take() else { return false };
        if matches!(press.key, K::Char('y' | 'Y')) { self.open_link(&link.href); } else { self.toast("link not opened"); }
        true
    }

    fn open_link(&mut self, href: &str) {
        match crate::platform::open_url(href) {
            Ok(()) => self.toast(format!("opened {}", host(href))),
            Err(reason) => self.error(reason),
        }
    }
}

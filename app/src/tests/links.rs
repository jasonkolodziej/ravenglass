//! Links in the HTML reader: the pointer over the painted card, the URL in the status line, and
//! what a click opens (recorded here; tests never start a browser).

use super::ui::{keyed, press, scene, settle, text_in};
use crate::links::host;
use crate::platform;
use crate::state::AppState;
use ravenglass_engine::{Command, Link};
use rune::{Key, Runtime};

const FRAME: f64 = 1.0 / 60.0;

/// A point of the laid-out newsletter that is on a link, in CSS pixels, found by asking the
/// engine the way the pointer does.
fn a_link(rt: &mut Runtime<AppState>) -> (f32, f32, Link) {
    for y in (8..600).step_by(6) {
        for x in (8..600).step_by(12) {
            let (x, y) = (x as f32, y as f32);
            rt.state.dispatch(Command::HtmlPointer { view: crate::fold::HTML_VIEW, x, y });
            if let Some(link) = rt.state.links.hover.take() { return (x, y, link); }
        }
    }
    panic!("the newsletter has a link near its top");
}

#[test]
fn hovering_a_link_shows_its_url_and_a_click_opens_it() {
    let mut rt = scene("html-newsletter", (1400.0, 880.0));
    settle(&mut rt);
    let (x, y, link) = a_link(&mut rt);
    assert!(link.href.starts_with("http"), "{}", link.href);
    let g = rt.state.frame.get().grid;
    let card = keyed(&rt.elements(), "reader-html").expect("the HTML card").rect;
    // The tiles start one cell in from the card's left edge.
    let (wx, wy) = (card[0] + g.cell_w + x, card[1] + y);
    rt.pointer_move(wx, wy);
    rt.frame(FRAME);
    assert_eq!(rt.state.links.hover.as_ref(), Some(&link), "the pointer's place reached the engine");
    let status = text_in(&rt.elements(), "status");
    assert!(status.contains(&link.href.chars().take(24).collect::<String>()), "{status}");

    rt.pointer_down(wx, wy);
    rt.pointer_up(wx, wy);
    rt.frame(FRAME);
    if link.text_names_another_host() {
        assert_eq!(rt.state.links.confirm.as_ref(), Some(&link));
    } else {
        assert!(platform::recorded().contains(&link.href), "the click opened it through platform::open_url");
        assert_eq!(text_in(&rt.elements(), "toast"), format!("opened {}", host(&link.href)));
    }

    // Away from every link the status line shows the mailbox again.
    rt.pointer_move(card[0] + g.cell_w + 2.0, card[1] + 2.0);
    rt.frame(FRAME);
    if rt.state.links.hover.is_none() { assert!(text_in(&rt.elements(), "status").contains("iCloud/Inbox")); }
}

#[test]
fn a_link_whose_text_names_another_site_asks_first() {
    let mut rt = scene("html-newsletter", (1400.0, 880.0));
    let phish = Link { href: "https://paypa1.example/login?step=1".into(), text: "paypal.com".into() };
    assert!(phish.text_names_another_host());
    let ask = |rt: &mut Runtime<AppState>| {
        rt.state.links.hover = Some(phish.clone());
        rt.state.html_click();
        // The state changed outside a handler: have the view run again.
        rt.settle_all();
        rt.frame(FRAME);
        text_in(&rt.elements(), "status")
    };
    let status = ask(&mut rt);
    assert!(status.contains("open paypa1.example? y/n"), "{status}");
    assert!(!platform::recorded().contains(&phish.href), "nothing opens before the answer");
    // Any key but `y` is a no, and is not a list key while the question is up.
    let sel = rt.state.ws().sel;
    press(&mut rt, Key::Text("j".into()));
    assert_eq!((rt.state.ws().sel, rt.state.links.confirm.is_none()), (sel, true));
    assert!(!platform::recorded().contains(&phish.href));
    assert_eq!(text_in(&rt.elements(), "toast"), "link not opened");
    ask(&mut rt);
    press(&mut rt, Key::Text("y".into()));
    assert!(platform::recorded().contains(&phish.href));
    assert_eq!(text_in(&rt.elements(), "toast"), "opened paypa1.example");
}

#[test]
fn only_web_and_mail_links_leave_the_app() {
    assert!(platform::open_link("file:///etc/passwd").is_err());
    assert!(platform::open_link("javascript:alert(1)").is_err());
    assert!(platform::open_link("  https://status.example/notes ").is_ok());
    assert!(platform::recorded().contains(&"https://status.example/notes".to_owned()));
    assert_eq!(host("https://user@ferries.example:8443/norrland?x#y"), "ferries.example:8443");
    assert_eq!(host("mailto:odin@asgard.example"), "asgard.example");
}

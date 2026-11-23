//! The right-hand icon rail: thin stroke icons for global actions, each a labelled button that
//! runs the same `Action` its key does.

use super::{RAIL_COLS, Ui};
use crate::actions::Action;
use crate::panes::Axis;
use crate::state::AppState;
use rune::text::Icon;
use rune::{El, el, icon};

macro_rules! stroke_icon {
    ($name:ident, $id:literal, $body:literal) => {
        static $name: Icon = Icon {
            name: $id,
            view: 24.0,
            body: concat!(r#"<g fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">"#, $body, "</g>"),
        };
    };
}

stroke_icon!(COMPOSE, "rg-compose", r#"<path d="M4 20l1.2-4.4L16.6 4.2a2 2 0 0 1 2.9 0l.3.3a2 2 0 0 1 0 2.9L8.4 18.8z"/><path d="M14.5 6.5l3 3"/>"#);
stroke_icon!(SEARCH, "rg-search", r#"<circle cx="10.5" cy="10.5" r="6"/><path d="M15 15l5 5"/>"#);
stroke_icon!(INBOX, "rg-inbox", r#"<path d="M3.5 13l2.7-8h11.6l2.7 8v6h-17z"/><path d="M3.5 13h5l1 2.5h5l1-2.5h5"/>"#);
stroke_icon!(ARCHIVE, "rg-archive", r#"<path d="M3.5 5h17v4h-17z"/><path d="M5 9v10h14V9"/><path d="M10 13h4"/>"#);
stroke_icon!(FLAG, "rg-flag", r#"<path d="M6 21V4"/><path d="M6 5h12l-2.5 4 2.5 4H6"/>"#);
stroke_icon!(SPLIT, "rg-split", r#"<path d="M4 5h16v14H4z"/><path d="M12 5v14"/>"#);
stroke_icon!(VIEW, "rg-view", r#"<path d="M5 7h14"/><path d="M5 12h14"/><path d="M5 17h9"/>"#);
stroke_icon!(THEME, "rg-theme", r#"<circle cx="12" cy="12" r="3.2"/><path d="M12 3.5v2.3M12 18.2v2.3M3.5 12h2.3M18.2 12h2.3M6 6l1.6 1.6M16.4 16.4L18 18M6 18l1.6-1.6M16.4 7.6L18 6"/>"#);
stroke_icon!(HELP, "rg-help", r#"<circle cx="12" cy="12" r="8.5"/><path d="M9.6 9.6a2.5 2.5 0 1 1 3.6 2.2c-.8.5-1.2 1-1.2 1.9"/><path d="M12 16.7v.1"/>"#);

/// What the rail offers, top to bottom.
fn items(dark: bool) -> [(&'static Icon, &'static str, Action); 9] {
    [
        (&COMPOSE, "Compose", Action::Compose),
        (&SEARCH, "Search", Action::Search),
        (&INBOX, "Workspace 1", Action::Workspace(1)),
        (&ARCHIVE, "Archive", Action::Archive),
        (&FLAG, "Toggle flag", Action::ToggleFlag),
        (&SPLIT, "Split pane", Action::PaneSplit(Axis::Cols)),
        (&VIEW, "Toggle HTML / text view", Action::ToggleView),
        (&THEME, "Switch theme", Action::Theme(if dark { "frost".into() } else { "night".into() })),
        (&HELP, "Command palette", Action::Palette),
    ]
}

/// `rows` cells tall, [`RAIL_COLS`] wide, with a hairline on its left edge.
pub fn rail(ui: &Ui, rows: usize) -> El {
    let g = &ui.g;
    let p = &ui.p;
    let (w, h) = (g.x(RAIL_COLS), g.y(rows));
    // Icons are two cells wide in a two-row slot.
    let size = g.x(2);
    let dark = p.bg.r + p.bg.g + p.bg.b < p.text.r + p.text.g + p.text.b;
    let mut rail = el().key("rail").role("toolbar").label("Actions").size(w, h);
    rail = rail.child(el().absolute(0.0, 0.0).size(1.0, h).fill(p.border).inert());
    for (i, (glyph, label, action)) in items(dark).into_iter().enumerate() {
        if (i + 1) * 2 > rows { break; }
        let slot = el()
            .key(format!("rail:{}", glyph.name))
            .role("button")
            .label(label)
            .absolute(0.0, g.y(i * 2))
            .size(w, g.y(2))
            .center()
            .child(icon(glyph, size).color(p.muted))
            .on_press(move |s: &mut AppState| s.perform(action.clone()));
        rail = rail.child(slot);
    }
    rail
}

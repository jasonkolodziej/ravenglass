//! HTML views: one `HtmlView` state machine per reader pane, performed against the render
//! thread. Any failure ends in [`Event::HtmlFailed`], and the pane shows the text view.

use crate::api::{Event, ViewId};
use mail_blitz::{Failure, Images, Info, Options, Renderer};
use mail_core::machine::html_view::{self, Effect, State};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Html {
    pub renderer: Option<Renderer>,
    views: BTreeMap<ViewId, State>,
}

impl Html {
    /// `html` is the sanitized body, or `None` for a message without an HTML part.
    pub fn open(&mut self, view: ViewId, html: Option<String>, images: Images, options: Options, out: &mut Vec<Event>) {
        let (Some(renderer), Some(html)) = (&self.renderer, html) else {
            self.views.remove(&view);
            return out.push(Event::HtmlFailed { view, failure: Failure::Empty });
        };
        self.views.insert(view, html_view::open(options.width));
        let opened = renderer.open(view.0, html, images, options);
        self.laid_out(view, opened, out);
    }

    pub fn resize(&mut self, view: ViewId, width: u32, out: &mut Vec<Event>) {
        let Some(state) = self.views.get_mut(&view) else { return };
        for effect in html_view::step(state, html_view::Event::Width(width)) {
            if let (Effect::Layout { width }, Some(renderer)) = (effect, &self.renderer) {
                let resized = renderer.set_width(view.0, width);
                self.laid_out(view, resized, out);
            }
        }
    }

    pub fn tile(&mut self, view: ViewId, index: u32, out: &mut Vec<Event>) {
        let (Some(renderer), Some(state)) = (&self.renderer, self.views.get_mut(&view)) else { return };
        match renderer.tile(view.0, index) {
            Ok(Some(tile)) => {
                html_view::step(state, html_view::Event::TileDone);
                out.push(Event::HtmlTile { view, tile: Box::new(tile) });
            }
            Ok(None) => {}
            Err(failure) => self.fail(view, failure, out),
        }
    }

    pub fn pointer(&mut self, view: ViewId, x: f32, y: f32, out: &mut Vec<Event>) {
        let (Some(renderer), true) = (&self.renderer, self.views.contains_key(&view)) else { return };
        match renderer.link_at(view.0, x, y) {
            Ok(link) => out.push(Event::LinkHover { view, link }),
            Err(failure) => self.fail(view, failure, out),
        }
    }

    pub fn close(&mut self, view: ViewId) {
        self.views.remove(&view);
        if let Some(renderer) = &self.renderer { renderer.close(view.0); }
    }

    #[cfg(test)]
    pub fn state(&self, view: ViewId) -> Option<&State> { self.views.get(&view) }

    fn laid_out(&mut self, view: ViewId, result: Result<Info, Failure>, out: &mut Vec<Event>) {
        let (Ok(info), Some(state)) = (&result, self.views.get_mut(&view)) else {
            return self.fail(view, result.err().unwrap_or(Failure::Closed), out);
        };
        html_view::step(state, html_view::Event::LaidOut { height: info.height });
        out.push(Event::HtmlLaidOut { view, info: *info });
    }

    fn fail(&mut self, view: ViewId, failure: Failure, out: &mut Vec<Event>) {
        if let Some(state) = self.views.get_mut(&view) { html_view::step(state, html_view::Event::Crashed); }
        self.close(view);
        out.push(Event::HtmlFailed { view, failure });
    }
}

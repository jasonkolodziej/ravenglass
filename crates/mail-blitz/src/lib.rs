//! The Blitz host (architecture §7, stages 5–7): sanitized HTML in, RGBA tiles and link
//! hit-testing out. It knows nothing about Rune; the app hands tiles to `rune::Pixels`.
//!
//! Safety properties this crate keeps, on top of the sanitizer:
//! - **No network.** The only resources a document can load are the message's own `cid:` parts,
//!   `data:` images, and remote images the caller already fetched and passed in. Everything
//!   else is refused and counted, so a missed case in the sanitizer still cannot fetch.
//! - **Contained failure.** [`guarded`] turns a panic inside Blitz into [`Failure::Crashed`], and
//!   the reader falls back to the text view. (Release web builds abort on panic instead; there
//!   the renderer runs in a Worker the engine restarts.)
//!
//! Not here yet: a layout time limit (Blitz cannot be interrupted mid-layout, so [`HtmlView`]
//! only reports elapsed time), find-in-message highlights, and a GPU raster path.

mod net;
mod worker;
#[cfg(test)]
mod tests;

use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{BaseDocument, DocumentConfig, local_name};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use mail_render::InlinePart;
use std::collections::HashMap;
use std::sync::Arc;

pub use worker::{Info, Renderer};

/// Tiles are this many device pixels tall (`mail_core::machine::html_view::TILE_H`).
pub const TILE_H: u32 = 1024;
/// Caps from architecture §7: one image, and all images of one message.
pub const MAX_IMAGE_BYTES: usize = 10 << 20;
pub const MAX_MESSAGE_IMAGE_BYTES: usize = 40 << 20;
/// Documents taller than this (CSS px) are cut off; the text view has the rest.
pub const MAX_HEIGHT: f32 = 60_000.0;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Failure {
    #[error("the HTML renderer crashed")]
    Crashed,
    #[error("the message has no layout")]
    Empty,
    #[error("the view is closed")]
    Closed,
}

/// How an email is shown in a dark theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DarkMode {
    /// The email's own colours on a light card.
    Paper,
    /// Lightness inverted after painting, hues kept. Photos are inverted too.
    Darken,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Options {
    /// Layout width in CSS pixels (the reader pane's content width).
    pub width: u32,
    /// Device pixels per CSS pixel.
    pub scale: f32,
    pub dark: DarkMode,
    /// Default text, card and link colours as CSS colours, from the theme.
    pub text: String,
    pub paper: String,
    pub link: String,
}

impl Default for Options {
    fn default() -> Self {
        Self { width: 680, scale: 1.0, dark: DarkMode::Paper, text: "#17142a".into(), paper: "#ffffff".into(), link: "#1f6fbf".into() }
    }
}

/// What a document may load. Remote images are present only if the user allowed them and the
/// engine fetched them; with an empty map they show as placeholders.
#[derive(Clone, Debug, Default)]
pub struct Images {
    pub inline: Vec<InlinePart>,
    pub remote: HashMap<String, Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tile { pub index: u32, pub y: u32, pub width: u32, pub height: u32, pub rgba: Vec<u8> }

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Link { pub href: String, pub text: String }

/// One laid-out message. It cannot leave the thread that made it; other threads use [`Renderer`].
pub struct HtmlView {
    doc: BaseDocument,
    options: Options,
    refused: Arc<net::Refused>,
}

impl HtmlView {
    /// Parse, style and lay out. `html` must be `mail_render::sanitize` output.
    pub fn new(html: &str, images: Images, options: Options) -> Result<Self, Failure> {
        let show_remote = !images.remote.is_empty();
        let html = if show_remote { html.to_owned() } else { mail_render::placeholders(html) };
        let provider = Arc::new(net::MessageImages::new(images));
        let refused = provider.refused();
        let scale = options.scale.max(0.25);
        let config = DocumentConfig {
            viewport: Some(Viewport::new(device(options.width, scale), device(800, scale), scale, ColorScheme::Light)),
            net_provider: Some(provider),
            ..Default::default()
        };
        let sheet = format!(
            "html{{background:{paper};color:{text}}} body{{margin:16px;font-family:sans-serif;word-break:break-word}} img,table{{max-width:100%}} a{{color:{link}}}",
            paper = css_color(&options.paper), text = css_color(&options.text), link = css_color(&options.link),
        );
        // Our sheet is prepended as author CSS so the message's own styles override it.
        let mut doc = HtmlDocument::from_html(&format!("<style>{sheet}</style>{html}"), config).into_inner();
        doc.resolve(0.0);
        // Image bytes are delivered during the first pass; a second pass lays them out.
        doc.resolve(0.0);
        let view = Self { doc, options: Options { scale, ..options }, refused };
        if view.height() <= 0.0 { return Err(Failure::Empty); }
        Ok(view)
    }

    /// Lay out again at a new width, keeping the parsed document.
    pub fn set_width(&mut self, width: u32) {
        if width == self.options.width { return; }
        self.options.width = width;
        let scale = self.options.scale;
        self.doc.set_viewport(Viewport::new(device(width, scale), device(800, scale), scale, ColorScheme::Light));
        self.doc.resolve(0.0);
    }

    pub fn options(&self) -> &Options { &self.options }

    /// Laid-out height in CSS pixels.
    pub fn height(&self) -> f32 { self.doc.root_element().final_layout().size.height.min(MAX_HEIGHT) }

    /// Size in device pixels.
    pub fn size(&self) -> (u32, u32) { (device(self.options.width, self.options.scale), ((self.height() * self.options.scale).ceil() as u32).max(1)) }

    pub fn tile_count(&self) -> u32 { self.size().1.div_ceil(TILE_H) }

    /// Requests the document made and was refused (remote images, fonts, stylesheets).
    pub fn refused_requests(&self) -> u32 { self.refused.count() }

    /// Paint one tile. Tiles are independent, so only those near the viewport need painting.
    pub fn tile(&mut self, index: u32) -> Option<Tile> {
        let (width, total) = self.size();
        let y = index.checked_mul(TILE_H).filter(|y| *y < total)?;
        let height = TILE_H.min(total - y);
        let mut renderer = VelloCpuImageRenderer::new(width, height);
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        let scale = self.options.scale as f64;
        // The paint offset scrolls the document so this tile's top is the buffer's top.
        self.doc.set_viewport_scroll(blitz_dom::Point { x: 0.0, y: y as f64 / scale });
        renderer.render_to_vec(|scene| blitz_paint::paint_scene(scene, &mut self.doc, scale, width, height, 0, 0), &mut rgba);
        self.doc.set_viewport_scroll(blitz_dom::Point { x: 0.0, y: 0.0 });
        if self.options.dark == DarkMode::Darken { darken(&mut rgba); }
        Some(Tile { index, y, width, height, rgba })
    }

    /// The link under a point given in CSS pixels from the document's top-left.
    pub fn link_at(&self, x: f32, y: f32) -> Option<Link> {
        let mut node = self.doc.get_node(self.doc.hit(x, y)?.node_id);
        while let Some(n) = node {
            let anchor = n.element_data().filter(|e| e.name.local == local_name!("a"));
            if let Some(href) = anchor.and_then(|e| e.attr(local_name!("href"))) {
                return Some(Link { href: href.to_owned(), text: n.text_content().split_whitespace().collect::<Vec<_>>().join(" ") });
            }
            node = n.parent.and_then(|p| self.doc.get_node(p));
        }
        None
    }
}

impl Link {
    /// True when the visible text names a different host than the target: the reader asks
    /// before opening such a link (architecture §12, phishing links).
    pub fn text_names_another_host(&self) -> bool {
        let shown = self.text.trim().trim_start_matches("https://").trim_start_matches("http://");
        let looks_like_host = shown.contains('.') && !shown.contains(char::is_whitespace);
        looks_like_host && host(shown) != host(self.href.trim_start_matches("https://").trim_start_matches("http://"))
    }
}

fn host(url: &str) -> String {
    let host = url.split(['/', '?', '#']).next().unwrap_or("").rsplit('@').next().unwrap_or("");
    host.split(':').next().unwrap_or("").trim_start_matches("www.").to_ascii_lowercase()
}

fn device(css: u32, scale: f32) -> u32 { ((css as f32 * scale).round() as u32).max(1) }

/// Theme colours go into a stylesheet, so anything that is not a plain colour token is dropped.
fn css_color(value: &str) -> &str {
    let plain = !value.is_empty() && value.len() <= 32 && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '#' | '(' | ')' | ',' | '.' | ' ' | '%'));
    if plain { value } else { "inherit" }
}

/// Invert lightness and keep hue: white paper becomes black, dark text light, colours stay.
fn darken(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let (max, min) = (px[0].max(px[1]).max(px[2]) as i32, px[0].min(px[1]).min(px[2]) as i32);
        let shift = 255 - max - min;
        for c in &mut px[..3] { *c = (*c as i32 + shift).clamp(0, 255) as u8; }
    }
}

/// Run renderer code so that a panic becomes [`Failure::Crashed`] instead of taking the engine
/// down. The caller drops whatever the closure owned and shows the text view.
pub fn guarded<T>(work: impl FnOnce() -> Result<T, Failure>) -> Result<T, Failure> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or(Err(Failure::Crashed))
}

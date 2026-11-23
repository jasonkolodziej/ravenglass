//! The renderer on wasm, where there is no thread to give Blitz: the same [`Renderer`] API as
//! `worker.rs`, answered in place on the calling thread (the engine's).
//!
//! Containment differs from desktop: a wasm panic aborts instead of unwinding, so `guarded` cannot
//! catch one here. What still protects the page is checked before painting: the height cap
//! ([`crate::MAX_HEIGHT`]) and the per-image byte caps. The render Worker that the engine can
//! restart after a crash (architecture §7, "Isolation") replaces this file's host later.

use crate::worker::{Info, info, on};
use crate::{Failure, HtmlView, Images, Link, Options, Tile, guarded};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[cfg(target_feature = "atomics")]
compile_error!("mail-blitz's inline renderer assumes single-threaded wasm (no `atomics`)");

/// The open views. Blitz documents are not `Send`, but the engine that holds this handle is
/// (it is the same type on desktop, where it crosses to the engine thread).
#[derive(Clone, Default)]
pub struct Renderer { views: Rc<RefCell<HashMap<u64, HtmlView>>> }

// SAFETY: without the `atomics` target feature (refused above) wasm has exactly one thread, so
// the `Rc` can never be reached from two threads at once.
unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

impl Renderer {
    /// Never fails on wasm; the `Result` keeps the desktop signature.
    pub fn spawn() -> std::io::Result<Self> { Ok(Self::default()) }

    /// Lay out a message as view `view`, replacing any view with that id.
    pub fn open(&self, view: u64, html: String, images: Images, options: Options) -> Result<Info, Failure> {
        let mut views = self.views.borrow_mut();
        views.remove(&view);
        let opened = guarded(|| HtmlView::new(&html, images, options))?;
        let laid_out = info(&opened);
        views.insert(view, opened);
        Ok(laid_out)
    }

    pub fn set_width(&self, view: u64, width: u32) -> Result<Info, Failure> {
        let mut views = self.views.borrow_mut();
        let resized = on(&mut views, view, |v| { v.set_width(width); v.is_renderable().then(|| info(v)) });
        if matches!(resized, Ok(None)) { views.remove(&view); }
        resized.and_then(|info| info.ok_or(Failure::TooLarge))
    }

    pub fn tile(&self, view: u64, index: u32) -> Result<Option<Tile>, Failure> { on(&mut self.views.borrow_mut(), view, |v| v.tile(index)) }

    /// The link under a point in CSS pixels from the document's top-left.
    pub fn link_at(&self, view: u64, x: f32, y: f32) -> Result<Option<Link>, Failure> { on(&mut self.views.borrow_mut(), view, |v| v.link_at(x, y)) }

    pub fn close(&self, view: u64) { self.views.borrow_mut().remove(&view); }
}

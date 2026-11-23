//! The render thread. Blitz documents are not `Send`, so they live on one thread and everything
//! else holds a [`Renderer`] handle. Each request is answered before the next is taken, and a
//! panic inside Blitz costs only the view it happened in.

use crate::{Failure, HtmlView, Images, Link, Options, Tile, guarded};
use std::collections::HashMap;
use std::sync::mpsc::{Sender, channel};

/// What the reader needs to place tiles: sizes in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Info { pub width: u32, pub height: u32, pub tiles: u32, pub refused_requests: u32 }

type Reply<T> = Sender<Result<T, Failure>>;

enum Job {
    Open { view: u64, html: String, images: Images, options: Options, reply: Reply<Info> },
    SetWidth { view: u64, width: u32, reply: Reply<Info> },
    Tile { view: u64, index: u32, reply: Reply<Option<Tile>> },
    LinkAt { view: u64, x: f32, y: f32, reply: Reply<Option<Link>> },
    Close { view: u64 },
}

/// A handle to the render thread. Dropping every clone stops the thread.
#[derive(Clone)]
pub struct Renderer { jobs: Sender<Job> }

impl Renderer {
    pub fn spawn() -> std::io::Result<Self> {
        let (jobs, inbox) = channel::<Job>();
        std::thread::Builder::new().name("ravenglass-render".into()).spawn(move || {
            let mut views: HashMap<u64, HtmlView> = HashMap::new();
            for job in inbox {
                match job {
                    Job::Open { view, html, images, options, reply } => {
                        views.remove(&view);
                        let opened = guarded(|| HtmlView::new(&html, images, options));
                        let _ = reply.send(opened.map(|v| { let info = info(&v); views.insert(view, v); info }));
                    }
                    Job::SetWidth { view, width, reply } => {
                        let resized = on(&mut views, view, |v| { v.set_width(width); v.is_renderable().then(|| info(v)) });
                        if matches!(resized, Ok(None)) { views.remove(&view); }
                        let _ = reply.send(resized.and_then(|info| info.ok_or(Failure::TooLarge)));
                    }
                    Job::Tile { view, index, reply } => { let _ = reply.send(on(&mut views, view, |v| v.tile(index))); }
                    Job::LinkAt { view, x, y, reply } => { let _ = reply.send(on(&mut views, view, |v| v.link_at(x, y))); }
                    Job::Close { view } => { views.remove(&view); }
                }
            }
        })?;
        Ok(Self { jobs })
    }

    /// Lay out a message as view `view`, replacing any view with that id.
    pub fn open(&self, view: u64, html: String, images: Images, options: Options) -> Result<Info, Failure> {
        self.ask(|reply| Job::Open { view, html, images, options, reply })
    }

    pub fn set_width(&self, view: u64, width: u32) -> Result<Info, Failure> { self.ask(|reply| Job::SetWidth { view, width, reply }) }

    pub fn tile(&self, view: u64, index: u32) -> Result<Option<Tile>, Failure> { self.ask(|reply| Job::Tile { view, index, reply }) }

    /// The link under a point in CSS pixels from the document's top-left.
    pub fn link_at(&self, view: u64, x: f32, y: f32) -> Result<Option<Link>, Failure> { self.ask(|reply| Job::LinkAt { view, x, y, reply }) }

    pub fn close(&self, view: u64) { let _ = self.jobs.send(Job::Close { view }); }

    fn ask<T>(&self, job: impl FnOnce(Reply<T>) -> Job) -> Result<T, Failure> {
        let (reply, answer) = channel();
        self.jobs.send(job(reply)).map_err(|_| Failure::Crashed)?;
        answer.recv().unwrap_or(Err(Failure::Crashed))
    }
}

fn info(v: &HtmlView) -> Info {
    let (width, height) = v.size();
    Info { width, height, tiles: v.tile_count(), refused_requests: v.refused_requests() }
}

/// Run `work` on one view; a panic closes that view.
fn on<T>(views: &mut HashMap<u64, HtmlView>, view: u64, work: impl FnOnce(&mut HtmlView) -> T) -> Result<T, Failure> {
    let v = views.get_mut(&view).ok_or(Failure::Closed)?;
    let result = guarded(|| Ok(work(v)));
    if result.is_err() { views.remove(&view); }
    result
}

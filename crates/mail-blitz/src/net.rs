//! The document's only way to load anything: a provider that serves the message's own parts
//! and pre-fetched, user-allowed images, and refuses every other request.

use crate::{Images, MAX_IMAGE_BYTES, MAX_MESSAGE_IMAGE_BYTES};
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Default)]
pub struct Refused(AtomicU32);

impl Refused {
    pub fn count(&self) -> u32 { self.0.load(Ordering::Relaxed) }
}

pub struct MessageImages {
    images: Images,
    served: AtomicUsize,
    refused: Arc<Refused>,
}

impl MessageImages {
    pub fn new(images: Images) -> Self { Self { images, served: AtomicUsize::new(0), refused: Arc::default() } }

    pub fn refused(&self) -> Arc<Refused> { self.refused.clone() }

    fn find(&self, url: &str) -> Option<&[u8]> {
        if let Some(rest) = url.strip_prefix("cid:") {
            // `cid://<message>/<content-id>`; the id itself may contain `/`, so match by suffix.
            let decoded = percent_decode(rest);
            return self.images.inline.iter().find(|p| decoded.ends_with(&format!("/{}", p.content_id)) || decoded == p.content_id).map(|p| p.bytes.as_slice());
        }
        self.images.remote.get(url).map(Vec::as_slice)
    }
}

impl NetProvider for MessageImages {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url.as_str();
        let found = self.find(url).filter(|bytes| {
            bytes.len() <= MAX_IMAGE_BYTES && self.served.fetch_add(bytes.len(), Ordering::Relaxed) + bytes.len() <= MAX_MESSAGE_IMAGE_BYTES
        });
        match found {
            // Delivered at once: the bytes are already in memory, so layout never waits.
            Some(bytes) => handler.bytes(url.to_owned(), Bytes::copy_from_slice(bytes)),
            None => { self.refused.0.fetch_add(1, Ordering::Relaxed); }
        }
    }
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        let escaped = (b == b'%').then(|| { let mut rest = bytes.clone(); Some((rest.next()?, rest.next()?)) }).flatten();
        match escaped.and_then(|(h, l)| u8::from_str_radix(std::str::from_utf8(&[h, l]).ok()?, 16).ok()) {
            Some(byte) => { out.push(byte); bytes.next(); bytes.next(); }
            None => out.push(b),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

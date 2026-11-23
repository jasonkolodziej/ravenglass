//! What the reader would make of a message, described without any of its content: sizes, the
//! MIME tree as types, and what the HTML sanitizer found.

use mail_core::MessageId;
use mail_parser::{MessageParser, MessagePart};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Shape {
    pub bytes: usize,
    /// Each part's type, indented by depth: `multipart/alternative`, `  text/plain`, …
    pub parts: Vec<String>,
    pub has_text: bool,
    pub has_html: bool,
    /// Remote images left in the sanitized HTML (shown only if the user allows).
    pub remote_images: u32,
    pub trackers_removed: u32,
    pub attachments: usize,
    pub inline_parts: usize,
    /// The reader's text view: lines and links, as counts.
    pub text_lines: usize,
    pub links: usize,
}

/// `None` when the bytes are not a message Ravenglass can read.
pub fn shape(raw: &[u8]) -> Option<Shape> {
    let parsed = mail_render::parse(raw).ok()?;
    let mut shape = Shape { bytes: raw.len(), has_text: parsed.text.is_some(), has_html: parsed.html.is_some(), attachments: parsed.attachments.len(), inline_parts: parsed.inline.len(), ..Shape::default() };
    let view = parsed.body(&MessageId::from("check"), mail_render::Prefer::Html);
    if let mail_render::Body::Html { html, .. } = &view {
        shape.remote_images = html.remote_images;
        shape.trackers_removed = html.trackers_removed;
    }
    shape.text_lines = view.text().lines.len();
    shape.links = view.text().links().count();
    if let Some(message) = MessageParser::default().parse(raw) {
        tree(&message.root_part(), 0, &mut shape.parts);
    }
    Some(shape)
}

/// The MIME tree below `part`, as content types only.
fn tree(part: &MessagePart<'_>, depth: usize, out: &mut Vec<String>) {
    // Deeper than any real message; a hostile one cannot recurse forever.
    if depth > 16 || out.len() > 200 { return; }
    let kind = part.content_type().map_or_else(|| "text/plain".to_owned(), |t| format!("{}/{}", t.ctype(), t.subtype().unwrap_or("?")).to_ascii_lowercase());
    out.push(format!("{}{kind}", "  ".repeat(depth)));
    for child in part.children() { tree(&child, depth + 1, out); }
    if let Some(inner) = part.nested() { out.push(format!("{}(attached message, {} parts)", "  ".repeat(depth + 1), inner.parts().count())); }
}

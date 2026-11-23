//! MIME → body model (architecture §7, stages 1–4). Pure and synchronous: the engine calls it off
//! the UI thread, `mail-blitz` lays out the sanitized HTML, and the text view is always available.
//!
//! Message content is hostile input. Nothing here fetches anything, and HTML leaves this crate
//! only through [`sanitize`].

mod css;
mod html_text;
mod sanitize;
mod text;
#[cfg(test)]
mod tests;

pub use html_text::html_to_text;
pub use sanitize::{SanitizedHtml, sanitize};
pub use text::{Line, QUOTE_FOLD_AFTER, Span, TextBody};

use mail_core::{Address, Attachment, MessageId, Timestamp};
use mail_parser::{MessageParser, PartKind};

/// Messages larger than this are not parsed (architecture §12).
pub const MAX_MESSAGE_BYTES: usize = 50 << 20;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RenderError {
    #[error("message is larger than {} MB", MAX_MESSAGE_BYTES >> 20)]
    TooLarge,
    #[error("not an RFC 5322 message")]
    Malformed,
}

/// An inline part a body refers to as `cid:<content_id>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlinePart { pub content_id: String, pub mime: String, pub bytes: Vec<u8> }

/// A decoded message: headers the reader shows, both body alternatives as sent, and its parts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub subject: String,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: Option<Timestamp>,
    /// The `text/plain` body, if the sender wrote one.
    pub text: Option<String>,
    /// The `text/html` body, unsanitized. Never show it; pass it through [`sanitize`].
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
    pub inline: Vec<InlinePart>,
}

pub fn parse(raw: &[u8]) -> Result<Parsed, RenderError> {
    if raw.len() > MAX_MESSAGE_BYTES { return Err(RenderError::TooLarge); }
    let msg = MessageParser::default().parse(raw).ok_or(RenderError::Malformed)?;
    let addresses = |list: Option<mail_parser::AddressList<'_>>| -> Vec<Address> {
        list.map(|l| l.mailboxes().filter_map(|m| Some(Address { name: m.name().map(str::to_owned), email: m.address()?.to_owned() })).collect()).unwrap_or_default()
    };
    let join = |parts: Vec<String>| (!parts.is_empty()).then(|| parts.join("\n"));
    let text = join(msg.text_body().filter(|p| matches!(p.kind(), PartKind::Text)).filter_map(|p| p.text().map(|t| t.into_owned())).collect());
    let html = join(msg.html_body().filter(|p| matches!(p.kind(), PartKind::Html)).filter_map(|p| p.text().map(|t| t.into_owned())).collect());
    let mime = |p: &mail_parser::MessagePart<'_>| {
        p.content_type().map_or_else(|| "application/octet-stream".to_owned(), |c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")).to_ascii_lowercase())
    };
    let cid = |p: &mail_parser::MessagePart<'_>| p.content_id().map(|c| c.trim_matches(['<', '>']).to_owned());
    let mut parsed = Parsed {
        subject: msg.subject().unwrap_or_default().to_owned(),
        from: addresses(msg.from()).into_iter().next(),
        to: addresses(msg.to()),
        cc: addresses(msg.cc()),
        date: msg.date().map(|d| d.to_timestamp()),
        text,
        html,
        ..Default::default()
    };
    for part in msg.attachments().chain(msg.html_body()).filter(|p| matches!(p.kind(), PartKind::Binary | PartKind::InlineBinary)) {
        let content_id = cid(&part);
        if let Some(content_id) = &content_id
            && !parsed.inline.iter().any(|i| &i.content_id == content_id)
        {
            parsed.inline.push(InlinePart { content_id: content_id.clone(), mime: mime(&part), bytes: part.decoded().into_owned() });
        }
        let shown_inline = content_id.is_some() && part.content_disposition().is_none_or(|d| d.is_inline());
        if !shown_inline {
            let name = part.attachment_name().unwrap_or("untitled").to_owned();
            parsed.attachments.push(Attachment { name, mime: mime(&part), size: part.decoded_len() as u64, content_id });
        }
    }
    Ok(parsed)
}

/// Which alternative the reader opens with (a per-account and per-user setting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefer { Html, Plain }

/// What the reader shows. The text view exists for every message: it is the fallback when the
/// HTML renderer fails and the target of the `t` toggle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Text(TextBody),
    Html { html: SanitizedHtml, text: TextBody },
}

impl Body {
    pub fn text(&self) -> &TextBody { match self { Body::Text(t) | Body::Html { text: t, .. } => t } }
}

impl Parsed {
    pub fn body(&self, message: &MessageId, prefer: Prefer) -> Body {
        let sanitized = self.html.as_deref().map(|h| sanitize(h, message));
        let text = match (&self.text, &sanitized) {
            (Some(t), _) => TextBody::from_plain(t),
            (None, Some(s)) => TextBody::from_plain(&html_to_text(&s.html)),
            (None, None) => TextBody::default(),
        };
        match (sanitized, prefer) {
            (Some(html), Prefer::Html) => Body::Html { html, text },
            _ => Body::Text(text),
        }
    }

    /// Text for the search index: the plain part, else text extracted from the sanitized HTML.
    pub fn index_text(&self, message: &MessageId) -> String {
        match (&self.text, &self.html) {
            (Some(t), _) => t.clone(),
            (None, Some(h)) => html_to_text(&sanitize(h, message).html),
            (None, None) => String::new(),
        }
    }
}

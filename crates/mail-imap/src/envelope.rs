//! Building an [`Envelope`] from what one FETCH returns: selected header fields, the first bytes
//! of the body for the preview, FLAGS, INTERNALDATE and BODYSTRUCTURE. Pure functions.

use async_imap::imap_proto::{BodyContentCommon, BodyContentSinglePart, BodyStructure, ContentEncoding};
use mail_core::{Address, Attachment, Envelope, Flags, FolderId, MessageId, ThreadId, Timestamp};
use mail_parser::MessageParser;

/// Characters of body text kept for the list row.
pub const PREVIEW_CHARS: usize = 200;
/// Body bytes fetched to build it: enough to get past a MIME preamble and some HTML boilerplate.
pub const PREVIEW_BYTES: usize = 3072;
/// The header fields an envelope needs. The MIME ones let the body snippet be decoded.
pub const HEADER_FIELDS: &str = "FROM TO CC SUBJECT DATE MESSAGE-ID IN-REPLY-TO REFERENCES MIME-VERSION CONTENT-TYPE CONTENT-TRANSFER-ENCODING";

const FLAG_NAMES: [(Flags, &str); 5] = [(Flags::SEEN, "\\Seen"), (Flags::FLAGGED, "\\Flagged"), (Flags::ANSWERED, "\\Answered"), (Flags::DRAFT, "\\Draft"), (Flags::DELETED, "\\Deleted")];

/// The system flags among a FETCH's flag names. `\Recent` and keywords are not tracked.
pub fn flags_from<'a>(names: impl IntoIterator<Item = &'a str>) -> Flags {
    let mut out = Flags::empty();
    for name in names {
        if let Some((flag, _)) = FLAG_NAMES.iter().find(|(_, known)| known.eq_ignore_ascii_case(name)) { out.insert(*flag); }
    }
    out
}

/// INTERNALDATE (`23-Nov-2026 04:15:00 +0000`, day possibly space-padded) as a timestamp.
pub fn internal_date(text: &str) -> Option<Timestamp> {
    let mut parts = text.split_whitespace();
    let (date, time, zone) = (parts.next()?, parts.next()?, parts.next()?);
    let mut date = date.split('-');
    let (day, month, year) = (date.next()?.parse::<i64>().ok()?, date.next()?, date.next()?.parse::<i64>().ok()?);
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| m.eq_ignore_ascii_case(month))? as i64 + 1;
    let mut time = time.split(':').map(|n| n.parse::<i64>().ok());
    let (hour, minute, second) = (time.next()??, time.next()??, time.next()??);
    let offset = zone.get(1..).filter(|digits| digits.len() == 4).and_then(|digits| digits.parse::<i64>().ok())?;
    let offset = (offset / 100 * 3600 + offset % 100 * 60) * if zone.starts_with('-') { -1 } else { 1 };
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 { return None; }
    // Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// The system flags in `flags` as a STORE or APPEND list, e.g. `(\Seen \Flagged)`. Bits that are
/// not IMAP flags are ignored. `None` when nothing is left.
pub fn flag_list(flags: Flags) -> Option<String> {
    let set: Vec<&str> = FLAG_NAMES.iter().filter(|(flag, _)| flags.contains(*flag)).map(|(_, name)| *name).collect();
    (!set.is_empty()).then(|| format!("({})", set.join(" ")))
}

/// What the server said about one message, besides its id.
pub struct Fetched<'a> {
    pub header: &'a [u8],
    /// The first [`PREVIEW_BYTES`] of the body, possibly cut mid-part or mid-character.
    pub text: &'a [u8],
    pub flags: Flags,
    /// INTERNALDATE, used when the Date header is missing or unreadable.
    pub received: Option<Timestamp>,
    pub attachments: Vec<Attachment>,
}

pub fn build(id: MessageId, folder: FolderId, fetched: Fetched<'_>) -> Envelope {
    let mut raw = Vec::with_capacity(fetched.header.len() + fetched.text.len() + 2);
    raw.extend_from_slice(fetched.header);
    if !raw.ends_with(b"\r\n\r\n") && !raw.ends_with(b"\n\n") { raw.extend_from_slice(b"\r\n"); }
    raw.extend_from_slice(fetched.text);
    let parsed = MessageParser::default().parse(&raw);
    let addresses = |list: Option<mail_parser::AddressList<'_>>| -> Vec<Address> {
        list.map(|l| l.mailboxes().filter_map(|m| Some(Address { name: m.name().map(str::to_owned), email: m.address()?.to_owned() })).collect()).unwrap_or_default()
    };
    let msg = parsed.as_ref();
    let from = addresses(msg.and_then(|m| m.from())).into_iter().next().unwrap_or(Address { name: None, email: String::new() });
    let thread = msg.and_then(|m| m.references().and_then(|r| r.first()).or_else(|| m.in_reply_to().and_then(|r| r.first())).or_else(|| m.message_id()));
    Envelope {
        // A message with no Message-ID is a thread of its own.
        thread: ThreadId(thread.map_or_else(|| id.0.clone(), |t| t.trim_matches(['<', '>', ' ']).to_owned())),
        id,
        folder,
        from,
        to: addresses(msg.and_then(|m| m.to())),
        cc: addresses(msg.and_then(|m| m.cc())),
        subject: msg.and_then(|m| m.subject()).unwrap_or_default().to_owned(),
        preview: msg.and_then(|m| m.body_text(0)).map(|text| preview(&text)).unwrap_or_default(),
        date: msg.and_then(|m| m.date()).map(|d| d.to_timestamp()).or(fetched.received).unwrap_or(0),
        flags: fetched.flags,
        attachments: fetched.attachments,
    }
}

/// One line of at most [`PREVIEW_CHARS`] characters: whitespace collapsed, and the split character
/// a cut body can end with dropped.
pub fn preview(text: &str) -> String {
    let mut out = String::with_capacity(PREVIEW_CHARS + 8);
    let mut count = 0;
    for word in text.split(|c: char| c.is_whitespace() || c == '\u{FFFD}' || c.is_control()).filter(|w| !w.is_empty()) {
        for c in (!out.is_empty()).then_some(' ').into_iter().chain(word.chars()) {
            if count == PREVIEW_CHARS { return out; }
            out.push(c);
            count += 1;
        }
    }
    out
}

/// The attachments a BODYSTRUCTURE describes, without fetching any of them. Sizes are estimates:
/// the server reports encoded bytes.
pub fn attachments(structure: &BodyStructure<'_>) -> Vec<Attachment> {
    let mut out = Vec::new();
    collect(structure, &mut out, 0);
    out
}

fn collect(structure: &BodyStructure<'_>, out: &mut Vec<Attachment>, depth: usize) {
    // Deeper nesting than any real message has; a hostile one cannot overflow the stack.
    if depth > 32 { return; }
    match structure {
        BodyStructure::Multipart { bodies, .. } => bodies.iter().for_each(|body| collect(body, out, depth + 1)),
        BodyStructure::Basic { common, other, .. } | BodyStructure::Text { common, other, .. } | BodyStructure::Message { common, other, .. } => {
            if let Some(attachment) = leaf(common, other) { out.push(attachment); }
        }
    }
}

fn leaf(common: &BodyContentCommon<'_>, other: &BodyContentSinglePart<'_>) -> Option<Attachment> {
    let param = |params: &Option<Vec<(std::borrow::Cow<'_, str>, std::borrow::Cow<'_, str>)>>, key: &str| {
        params.as_ref().and_then(|list| list.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.to_string()))
    };
    let disposition = common.disposition.as_ref().map(|d| d.ty.to_ascii_lowercase());
    let name = common.disposition.as_ref().and_then(|d| param(&d.params, "filename")).or_else(|| param(&common.ty.params, "name"));
    let kind = common.ty.ty.to_ascii_lowercase();
    let is_attachment = match disposition.as_deref() {
        Some("attachment") => true,
        // Inline parts are the body itself or images it shows.
        Some("inline") => false,
        _ => name.is_some() || (kind != "text" && other.id.is_none()),
    };
    if !is_attachment { return None; }
    let size = match other.transfer_encoding { ContentEncoding::Base64 => u64::from(other.octets) / 4 * 3, _ => u64::from(other.octets) };
    Some(Attachment {
        name: name.map_or_else(|| "attachment".to_owned(), |n| decode_words(&n)),
        mime: format!("{kind}/{}", common.ty.subtype.to_ascii_lowercase()),
        size,
        content_id: other.id.as_ref().map(|id| id.trim_matches(['<', '>']).to_owned()),
    })
}

/// File names arrive as raw header text; decode RFC 2047 encoded words (`=?utf-8?B?…?=`).
fn decode_words(value: &str) -> String {
    if !value.contains("=?") { return value.to_owned(); }
    let header = format!("Subject: {}\r\n\r\n", value.replace(['\r', '\n'], ""));
    MessageParser::default().parse(header.as_bytes()).and_then(|m| m.subject().map(str::to_owned)).unwrap_or_else(|| value.to_owned())
}

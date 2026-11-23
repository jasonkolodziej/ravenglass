//! The outgoing direction: a [`Draft`] to RFC 5322 bytes, and a received message to the draft
//! that answers it. Pure: the caller supplies the `Message-ID` and the date.

use crate::{Parsed, TextBody};
use mail_builder::MessageBuilder;
use mail_core::{Address, Draft, DraftId, AccountId, Timestamp};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReplyKind { Reply, ReplyAll, Forward }

fn address(a: &Address) -> mail_builder::headers::address::Address<'_> {
    match &a.name { Some(name) => (name.as_str(), a.email.as_str()).into(), None => a.email.as_str().into() }
}

fn addresses(list: &[Address]) -> mail_builder::headers::address::Address<'_> {
    mail_builder::headers::address::Address::new_list(list.iter().map(address).collect())
}

/// Serialize a draft. `Bcc` recipients are left out of the headers on purpose: they are named
/// only in the SMTP envelope ([`Draft::recipients`]).
pub fn build(draft: &Draft, message_id: &str, date: Timestamp) -> Vec<u8> {
    let mut msg = MessageBuilder::new().message_id(message_id.to_owned()).date(date).from(address(&draft.from)).subject(draft.subject.as_str()).text_body(draft.body.as_str());
    if !draft.to.is_empty() { msg = msg.to(addresses(&draft.to)); }
    if !draft.cc.is_empty() { msg = msg.cc(addresses(&draft.cc)); }
    if let Some(parent) = &draft.in_reply_to { msg = msg.in_reply_to(parent.clone()); }
    if !draft.references.is_empty() { msg = msg.references(draft.references.clone()); }
    for file in &draft.attachments { msg = msg.attachment(file.mime.as_str(), file.name.as_str(), file.bytes.as_slice()); }
    msg.write_to_vec().unwrap_or_default()
}

/// A `Message-ID` (without angle brackets) for a draft: unique per account, time and counter.
pub fn message_id(from: &Address, now: f64, counter: u64) -> String {
    let domain = from.email.rsplit_once('@').map_or("localhost", |(_, d)| d);
    format!("{:x}.{counter:x}.ravenglass@{domain}", (now.max(0.0) * 1000.0) as u64)
}

/// The draft that replies to or forwards `original`. `me` is the account's own address: it
/// becomes the sender and is left out of the recipients.
pub fn reply(original: &Parsed, kind: ReplyKind, id: DraftId, account: AccountId, me: &Address) -> Draft {
    let mut draft = Draft::new(id, account, me.clone());
    let not_me = |a: &&Address| !a.email.eq_ignore_ascii_case(&me.email);
    match kind {
        ReplyKind::Reply => draft.to = original.from.iter().cloned().collect(),
        ReplyKind::ReplyAll => {
            draft.to = original.from.iter().chain(original.to.iter().filter(not_me)).cloned().collect();
            draft.cc = original.cc.iter().filter(not_me).cloned().collect();
            let mut seen: Vec<String> = Vec::new();
            for list in [&mut draft.to, &mut draft.cc] {
                list.retain(|a| { let key = a.email.to_ascii_lowercase(); !seen.contains(&key) && { seen.push(key); true } });
            }
        }
        ReplyKind::Forward => {}
    }
    let (prefix, other) = if kind == ReplyKind::Forward { ("Fwd: ", "fw:") } else { ("Re: ", "re:") };
    let lower = original.subject.trim_start().to_ascii_lowercase();
    draft.subject = if lower.starts_with(&prefix.trim().to_ascii_lowercase()) || lower.starts_with(other) { original.subject.clone() } else { format!("{prefix}{}", original.subject) };
    if kind != ReplyKind::Forward {
        draft.in_reply_to = original.message_id.clone();
        draft.references = original.references.iter().chain(&original.message_id).cloned().collect();
    }
    draft.body = quote(original, kind);
    draft
}

fn quote(original: &Parsed, kind: ReplyKind) -> String {
    let text = match (&original.text, &original.html) {
        (Some(t), _) => t.clone(),
        (None, Some(h)) => crate::html_to_text(&crate::sanitize(h, &mail_core::MessageId::from("quote")).html),
        (None, None) => String::new(),
    };
    let who = original.from.as_ref().map_or("someone", Address::display);
    let when = original.date.map(date_line).unwrap_or_default();
    let lines = TextBody::from_plain(&text).lines;
    if kind == ReplyKind::Forward {
        let mut out = format!("\n\n---------- Forwarded message ----------\nFrom: {who}\n");
        if !when.is_empty() { out.push_str(&format!("Date: {when}\n")); }
        out.push_str(&format!("Subject: {}\n\n{}\n", original.subject, text.trim_end()));
        return out;
    }
    let mut out = if when.is_empty() { format!("\n\n{who} wrote:\n") } else { format!("\n\nOn {when}, {who} wrote:\n") };
    for line in lines {
        out.push_str(&">".repeat(line.quote_depth as usize + 1));
        let body = line.text();
        if !body.is_empty() { out.push(' '); out.push_str(&body); }
        out.push('\n');
    }
    out
}

/// `2026-11-13 16:44 UTC`.
fn date_line(t: Timestamp) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, secs % 3600 / 60)
}

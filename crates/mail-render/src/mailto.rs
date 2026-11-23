//! `mailto:` links (RFC 6068), as the OS hands them to the app when the user clicks one anywhere.
//! A link comes from outside the app, so it only ever fills in a draft for the person to read:
//! nothing is sent, and only the recipients, subject and body are taken. Every other header in a
//! link (`In-Reply-To`, `From`, ...) is ignored, as RFC 6068 §6 advises.

use mail_core::{Address, Draft};

/// Caps on what a link may carry.
const MAX_LINK: usize = 64 * 1024;
const MAX_RECIPIENTS: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mailto {
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: Option<String>,
    pub body: Option<String>,
}

impl Mailto {
    /// Parse a `mailto:` link. `None` for anything else, or a link too large to be one.
    pub fn parse(link: &str) -> Option<Mailto> {
        let link = link.trim();
        if link.len() > MAX_LINK { return None; }
        let rest = link.get(..7).filter(|s| s.eq_ignore_ascii_case("mailto:")).map(|_| &link[7..])?;
        let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
        let mut out = Mailto::default();
        out.add(Field::To, &decode(to)?);
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = decode(value)?;
            match decode(key)?.to_ascii_lowercase().as_str() {
                "to" => out.add(Field::To, &value),
                "cc" => out.add(Field::Cc, &value),
                "bcc" => out.add(Field::Bcc, &value),
                "subject" => out.subject = Some(value.chars().filter(|c| !c.is_control()).collect()),
                // Line breaks in a body arrive as %0D%0A; the draft uses plain newlines.
                "body" => out.body = Some(value.replace("\r\n", "\n")),
                _ => {}
            }
        }
        Some(out)
    }

    fn add(&mut self, field: Field, list: &str) {
        for one in list.split(',').map(str::trim).filter(|a| !a.is_empty()) {
            if self.to.len() + self.cc.len() + self.bcc.len() >= MAX_RECIPIENTS { return; }
            let Some(address) = address(one) else { continue };
            let list = match field { Field::To => &mut self.to, Field::Cc => &mut self.cc, Field::Bcc => &mut self.bcc };
            if !list.iter().any(|a| a.email.eq_ignore_ascii_case(&address.email)) { list.push(address); }
        }
    }

    /// Fill a fresh draft. Recipients are added to any already there.
    pub fn apply(&self, draft: &mut Draft) {
        draft.to.extend(self.to.iter().cloned());
        draft.cc.extend(self.cc.iter().cloned());
        draft.bcc.extend(self.bcc.iter().cloned());
        if let Some(subject) = &self.subject { draft.subject = subject.clone(); }
        if let Some(body) = &self.body { draft.body = body.clone(); }
    }
}

enum Field { To, Cc, Bcc }

/// `a@b.example`, or `Name <a@b.example>`. Anything that is not one address is dropped.
fn address(text: &str) -> Option<Address> {
    let (name, email) = match text.rsplit_once('<') {
        Some((name, rest)) => (Some(name.trim().trim_matches('"').trim()).filter(|n| !n.is_empty()), rest.strip_suffix('>')?.trim()),
        None => (None, text),
    };
    let (local, domain) = email.split_once('@')?;
    let ok = |s: &str| !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';' | '"' | '@'));
    (ok(local) && ok(domain) && domain.contains('.')).then(|| Address { name: name.map(str::to_owned), email: email.to_owned() })
}

/// Percent-decoding to UTF-8. `None` for a bad escape or bytes that are not UTF-8.
fn decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b != b'%' { out.push(b); continue; }
        let hex = [bytes.next()?, bytes.next()?];
        out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
    }
    String::from_utf8(out).ok()
}

/// Percent-encode a link so it can travel inside another one (`ravenglass://mailto?u=…`).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') { out.push(b as char); } else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

/// The `mailto:` link inside one of the app's own `ravenglass://mailto?u=…` links, or the link
/// itself when it already is one.
pub fn unwrap_link(link: &str) -> Option<String> {
    let link = link.trim();
    if link.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:")) { return Some(link.to_owned()); }
    let query = link.strip_prefix("ravenglass://mailto?")?;
    query.split('&').find_map(|p| p.strip_prefix("u=")).and_then(decode).filter(|inner| inner.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:")))
}

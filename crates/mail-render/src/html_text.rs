//! Sanitized HTML → plain text for the text view, the search index and the fallback when the
//! HTML renderer fails. A stand-in for the planned HTML → Markdown conversion: it keeps reading
//! order, links, list bullets and quote depth, and drops layout.

use crate::sanitize::{attributes, tag_end};

/// Tags that end a paragraph (a blank line) and tags that only end a line.
const PARAGRAPHS: [&str; 13] = ["p", "table", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "blockquote", "pre", "hr"];
const LINES: [&str; 9] = ["div", "tr", "li", "center", "section", "article", "header", "footer", "br"];

/// `html` must be [`crate::sanitize`] output: the scan relies on its normalized, escaped form.
pub fn html_to_text(html: &str) -> String {
    let mut w = Writer::default();
    let mut rest = html;
    let mut link: Option<(String, usize)> = None;
    while let Some(lt) = rest.find('<') {
        w.text(&decode(&rest[..lt]));
        rest = &rest[lt..];
        let (tag, after) = rest.split_at(tag_end(rest));
        rest = after;
        let closing = tag.starts_with("</");
        let name = tag.trim_start_matches(['<', '/']).split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or("").to_ascii_lowercase();
        match (name.as_str(), closing) {
            ("style", false) => rest = rest.find("</style>").map_or("", |i| &rest[i..]),
            ("blockquote", false) => { w.end_paragraph(); w.quote += 1; }
            ("blockquote", true) => { w.end_paragraph(); w.quote = w.quote.saturating_sub(1); }
            ("li", false) => { w.end_line(); w.text("• "); }
            ("td" | "th", true) => w.text(" "),
            ("img", false) => {
                if let Some((_, alt)) = attributes(tag).into_iter().find(|(n, v)| *n == "alt" && !v.trim().is_empty()) { w.text(&format!("[{}]", decode(alt))); }
            }
            ("a", false) => link = attributes(tag).into_iter().find(|(n, _)| *n == "href").map(|(_, v)| (decode(v), w.out.len())),
            ("a", true) => {
                if let Some((href, start)) = link.take() {
                    let shown = w.out.get(start..).unwrap_or("").trim().to_owned();
                    if !href.is_empty() && shown != href && !href.starts_with('#') { w.text(&format!(" ({href})")); }
                }
            }
            (n, _) if PARAGRAPHS.contains(&n) => w.end_paragraph(),
            (n, _) if LINES.contains(&n) => w.end_line(),
            _ => {}
        }
    }
    w.text(&decode(rest));
    w.finish()
}

#[derive(Default)]
struct Writer { out: String, quote: usize }

impl Writer {
    fn text(&mut self, text: &str) {
        for word in text.split_inclusive(char::is_whitespace) {
            let body = word.trim_end();
            if !body.is_empty() {
                if self.out.is_empty() || self.out.ends_with('\n') { self.out.push_str(&"> ".repeat(self.quote)); }
                self.out.push_str(body);
            }
            if body.len() != word.len() && !self.out.is_empty() && !self.out.ends_with([' ', '\n']) { self.out.push(' '); }
        }
    }

    fn end_line(&mut self) {
        while self.out.ends_with(' ') { self.out.pop(); }
        if !self.out.is_empty() && !self.out.ends_with('\n') { self.out.push('\n'); }
    }

    /// End the line and leave one blank line, never more.
    fn end_paragraph(&mut self) {
        self.end_line();
        if !self.out.is_empty() && !self.out.ends_with("\n\n") { self.out.push('\n'); }
    }

    fn finish(mut self) -> String {
        while self.out.ends_with([' ', '\n']) { self.out.pop(); }
        self.out
    }
}

/// The entities the sanitizer's serializer writes, plus numeric ones.
fn decode(s: &str) -> String {
    if !s.contains('&') { return s.to_owned(); }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest[..rest.len().min(12)].find(';');
        let decoded = end.and_then(|end| match &rest[1..end] {
            "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'), "apos" => Some('\''), "nbsp" => Some(' '),
            n => n.strip_prefix("#x").or_else(|| n.strip_prefix("#X")).map(|h| u32::from_str_radix(h, 16))
                .or_else(|| n.strip_prefix('#').map(str::parse::<u32>))
                .and_then(|r| r.ok()).and_then(char::from_u32),
        });
        match (decoded, end) {
            (Some(ch), Some(end)) => { out.push(ch); rest = &rest[end + 1..]; }
            _ => { out.push('&'); rest = &rest[1..]; }
        }
    }
    out.push_str(rest);
    out
}

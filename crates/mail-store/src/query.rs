//! The search language (architecture §6): free text plus `from:`, `to:`, `subject:`,
//! `has:attachment`, `is:unread`, `is:flagged`, `in:<folder>`, `before:`/`after:` (ISO dates).
//! Parsing is pure; `Store::search` turns a [`Query`] into FTS5 `MATCH` plus SQL filters.

use mail_core::Timestamp;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: Vec<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub subject: Vec<String>,
    pub has_attachment: bool,
    pub unread: Option<bool>,
    pub flagged: Option<bool>,
    pub folder: Option<String>,
    /// Exclusive upper bound.
    pub before: Option<Timestamp>,
    /// Inclusive lower bound.
    pub after: Option<Timestamp>,
}

/// One piece of the query as typed, so the UI can highlight operator tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token { pub start: usize, pub end: usize, pub operator: bool }

impl Query {
    /// Never fails: an operator with a bad value (`before:tomorrow`) is searched as plain text.
    pub fn parse(input: &str) -> Self { Self::parse_tokens(input).0 }

    pub fn parse_tokens(input: &str) -> (Self, Vec<Token>) {
        let mut q = Query::default();
        let mut tokens = Vec::new();
        for (start, end) in split(input) {
            let word = &input[start..end];
            let operator = q.apply(word);
            if !operator { q.text.push(unquote(word).to_owned()); }
            tokens.push(Token { start, end, operator });
        }
        (q, tokens)
    }

    fn apply(&mut self, word: &str) -> bool {
        let Some((key, value)) = word.split_once(':') else { return false };
        let value = unquote(value);
        if value.is_empty() { return false; }
        match (key.to_ascii_lowercase().as_str(), value.to_ascii_lowercase().as_str()) {
            ("from", _) => self.from.push(value.into()),
            ("to", _) => self.to.push(value.into()),
            ("subject", _) => self.subject.push(value.into()),
            ("in", _) => self.folder = Some(value.into()),
            ("has", "attachment" | "attachments") => self.has_attachment = true,
            ("is", "unread") => self.unread = Some(true),
            ("is", "read") => self.unread = Some(false),
            ("is", "flagged") => self.flagged = Some(true),
            ("is", "unflagged") => self.flagged = Some(false),
            ("before", v) => match parse_date(v) { Some(t) => self.before = Some(t), None => return false },
            ("after", v) => match parse_date(v) { Some(t) => self.after = Some(t), None => return false },
            _ => return false,
        }
        true
    }

    /// The FTS5 `MATCH` expression, or `None` when only SQL filters apply.
    pub fn fts_match(&self) -> Option<String> {
        let mut parts = Vec::new();
        let mut column = |col: &str, terms: &[String]| {
            for t in terms { for w in words(t) { parts.push(format!("{col}{}", phrase(&w))); } }
        };
        column("", &self.text);
        column("sender : ", &self.from);
        column("recipients : ", &self.to);
        column("subject : ", &self.subject);
        (!parts.is_empty()).then(|| parts.join(" AND "))
    }
}

/// Byte ranges of whitespace-separated words; double quotes keep spaces inside one word.
fn split(input: &str) -> Vec<(usize, usize)> {
    let (mut out, mut start, mut quoted) = (Vec::new(), None, false);
    for (i, ch) in input.char_indices() {
        if ch == '"' { quoted = !quoted; }
        match (ch.is_whitespace() && !quoted, start) {
            (true, Some(s)) => { out.push((s, i)); start = None; }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start { out.push((s, input.len())); }
    out
}

fn unquote(s: &str) -> &str { s.trim_matches('"') }

/// FTS5 tokens of a user term: split where the `unicode61` tokenizer would.
fn words(term: &str) -> Vec<String> {
    term.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect()
}

/// A quoted prefix term, so `git` finds `GitHub`. Quoting keeps user input out of FTS5 syntax.
fn phrase(word: &str) -> String { format!("\"{}\"*", word.replace('"', "")) }

/// `YYYY-MM-DD` → midnight UTC.
pub fn parse_date(s: &str) -> Option<Timestamp> {
    let mut it = s.split('-');
    let (y, m, d) = (it.next()?.parse::<i64>().ok()?, it.next()?.parse::<i64>().ok()?, it.next()?.parse::<i64>().ok()?);
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1970..=9999).contains(&y) { return None; }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400)
}

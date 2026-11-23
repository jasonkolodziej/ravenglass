//! The text view's model: lines with a quote depth, links found, long quotes folded.

/// Quoted runs longer than this many lines are folded ("Show quoted text").
pub const QUOTE_FOLD_AFTER: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Span { Text(String), Link(String) }

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Line {
    /// How many `>` markers the line had.
    pub quote_depth: u8,
    pub spans: Vec<Span>,
    /// Hidden until the reader opens the quote it belongs to.
    pub folded: bool,
}

impl Line {
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| match s { Span::Text(t) | Span::Link(t) => t.as_str() }).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TextBody { pub lines: Vec<Line> }

impl TextBody {
    pub fn from_plain(text: &str) -> Self {
        let mut lines: Vec<Line> = text.lines().map(parse_line).collect();
        while lines.last().is_some_and(|l| l.spans.is_empty()) { lines.pop(); }
        let mut run = 0;
        for line in &mut lines {
            run = if line.quote_depth > 0 { run + 1 } else { 0 };
            line.folded = run > QUOTE_FOLD_AFTER;
        }
        Self { lines }
    }

    pub fn folded_lines(&self) -> usize { self.lines.iter().filter(|l| l.folded).count() }

    pub fn links(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().flat_map(|l| &l.spans).filter_map(|s| match s { Span::Link(u) => Some(u.as_str()), Span::Text(_) => None })
    }
}

fn parse_line(raw: &str) -> Line {
    let mut rest = raw.trim_end();
    let mut quote_depth = 0u8;
    while let Some(r) = rest.trim_start_matches(' ').strip_prefix('>') {
        quote_depth = quote_depth.saturating_add(1);
        rest = r;
    }
    if quote_depth > 0 { rest = rest.strip_prefix(' ').unwrap_or(rest); }
    Line { quote_depth, spans: linkify(rest), folded: false }
}

const SCHEMES: [&str; 3] = ["https://", "http://", "mailto:"];

fn linkify(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(start) = SCHEMES.iter().filter_map(|s| rest.find(s)).min() {
        let tail = &rest[start..];
        let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"')).unwrap_or(tail.len());
        // Sentence punctuation after a URL is not part of it.
        let url = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '\'']);
        if SCHEMES.iter().any(|s| s.starts_with(url)) {
            // A bare scheme is just text.
            let (head, after) = rest.split_at(start + url.len());
            push_text(&mut spans, head);
            rest = after;
            continue;
        }
        push_text(&mut spans, &rest[..start]);
        spans.push(Span::Link(url.to_owned()));
        rest = &tail[url.len()..];
    }
    push_text(&mut spans, rest);
    spans
}

fn push_text(spans: &mut Vec<Span>, text: &str) {
    if text.is_empty() { return; }
    match spans.last_mut() {
        Some(Span::Text(t)) => t.push_str(text),
        _ => spans.push(Span::Text(text.to_owned())),
    }
}

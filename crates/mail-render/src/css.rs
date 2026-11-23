//! CSS filtering for `style` attributes and `<style>` blocks. Emails need their CSS, so it is
//! kept, minus everything that could fetch (`@import`, remote `url()`) or escape the message's
//! box (`position: fixed`).

/// `url()` targets that never touch the network.
fn local_url(target: &str) -> bool {
    let t = target.trim().trim_matches(['"', '\'']).trim().to_ascii_lowercase();
    t.starts_with("cid:") || (t.starts_with("data:image/") && !t.starts_with("data:image/svg"))
}

pub fn filter_css(css: &str) -> String {
    // Escapes (`u\72l(`) and comments (`ur/**/l(`) can hide a function name from the scan below.
    let css = strip_comments(&unescape(css));
    let lower = css.to_ascii_lowercase();
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    while i < css.len() {
        let rest = &lower[i..];
        if rest.starts_with("@import") {
            i += rest.find([';', '\n']).map_or(rest.len(), |n| n + 1);
        } else if rest.starts_with("url(") {
            // An unclosed `url(` swallows the rest of the sheet, as it would in a browser.
            let Some(end) = rest.find(')') else { out.push_str("none"); break };
            if local_url(&css[i + 4..i + end]) { out.push_str(&css[i..=i + end]); } else { out.push_str("none"); }
            i += end + 1;
        } else if rest.starts_with("position") && rest[8..].trim_start().strip_prefix(':').is_some_and(|v| v.trim_start().starts_with("fixed")) {
            out.push_str("position:static");
            i += rest.find("fixed").unwrap_or(0) + 5;
        } else {
            let ch = css[i..].chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..].find("*/").map_or("", |end| &rest[start + 2 + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// Decode CSS escapes: `\72 ` is `r`, `\(` is `(`.
fn unescape(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' { out.push(ch); continue; }
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(char::is_ascii_hexdigit) { hex.extend(chars.next()); }
        if hex.is_empty() { out.extend(chars.next()); continue; }
        if chars.peek().is_some_and(|c| c.is_whitespace()) { chars.next(); }
        out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).filter(|c| *c != '\0').unwrap_or('\u{FFFD}'));
    }
    out
}

//! Stage 4: the allow-list sanitizer. `ammonia` removes scripts, frames, forms, handlers and
//! unknown URL schemes and writes normalized HTML; a second pass over that output filters CSS,
//! rewrites `cid:` images, drops tracking pixels and counts the remote images it left in place.
//! Remote images are not fetched here or anywhere by default: the renderer's net provider
//! refuses them until the user allows them.

use crate::css::filter_css;
use mail_core::MessageId;
use std::borrow::Cow;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SanitizedHtml {
    pub html: String,
    /// Remote images kept in the markup, to be loaded only if the user allows it.
    pub remote_images: u32,
    pub trackers_removed: u32,
}

const EXTRA_TAGS: [&str; 3] = ["style", "font", "center"];
const EXTRA_ATTRIBUTES: [&str; 15] = [
    "style", "class", "align", "valign", "bgcolor", "width", "height", "dir", "lang", "border", "cellpadding", "cellspacing", "color", "face", "size",
];
/// URL fragments of open-tracking endpoints, for pixels that are not sized 1×1.
const TRACKER_MARKERS: [&str; 4] = ["/track/open", "/wf/open", "/open.gif", "/o.gif"];

pub fn sanitize(html: &str, message: &MessageId) -> SanitizedHtml {
    let mut builder = ammonia::Builder::default();
    builder
        .rm_clean_content_tags(["style"])
        .add_tags(EXTRA_TAGS)
        .add_generic_attributes(EXTRA_ATTRIBUTES)
        .add_url_schemes(["cid", "data"])
        .attribute_filter(|element, attribute, value| {
            if attribute == "style" { return Some(Cow::Owned(filter_css(value))); }
            let data = value.trim_start().to_ascii_lowercase().starts_with("data:");
            let image = element == "img" && attribute == "src" && embedded_image(value);
            (!data || image).then_some(Cow::Borrowed(value))
        });
    let clean = builder.clean(html).to_string();
    post_pass(&clean, message)
}

fn embedded_image(url: &str) -> bool {
    let u = url.trim_start().to_ascii_lowercase();
    u.starts_with("data:image/") && !u.starts_with("data:image/svg")
}

/// Walk ammonia's output. Text is escaped there, so a literal `<img` or `<style` is always a tag.
fn post_pass(clean: &str, message: &MessageId) -> SanitizedHtml {
    let mut out = SanitizedHtml { html: String::with_capacity(clean.len()), ..Default::default() };
    let mut rest = clean;
    while let Some(lt) = rest.find('<') {
        out.html.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let end = tag_end(rest);
        let (tag, after) = rest.split_at(end);
        if tag.starts_with("<style") {
            let close = after.find("</style>").unwrap_or(after.len());
            out.html.push_str(tag);
            out.html.push_str(&filter_css(&after[..close]));
            rest = &after[close..];
            continue;
        }
        if tag.starts_with("<img") { image(tag, message, &mut out); } else { out.html.push_str(tag); }
        rest = after;
    }
    out.html.push_str(rest);
    out
}

/// Byte length of the tag starting at `s[0] == '<'`, honouring quoted attribute values.
pub(crate) fn tag_end(s: &str) -> usize {
    let mut quoted = false;
    for (i, b) in s.bytes().enumerate() {
        match b { b'"' => quoted = !quoted, b'>' if !quoted => return i + 1, _ => {} }
    }
    s.len()
}

/// Attributes of a normalized tag, values still entity-escaped.
pub(crate) fn attributes(tag: &str) -> Vec<(&str, &str)> {
    let mut attrs = Vec::new();
    let mut rest = tag.trim_start_matches('<').trim_end_matches('>').trim_end_matches('/');
    rest = rest.find(char::is_whitespace).map_or("", |i| &rest[i..]);
    while let Some(eq) = rest.find("=\"") {
        let name = rest[..eq].trim();
        let value_end = rest[eq + 2..].find('"').unwrap_or(rest.len() - eq - 2);
        attrs.push((name, &rest[eq + 2..eq + 2 + value_end]));
        rest = rest.get(eq + 2 + value_end + 1..).unwrap_or("");
    }
    attrs
}

fn image(tag: &str, message: &MessageId, out: &mut SanitizedHtml) {
    let attrs = attributes(tag);
    let get = |name: &str| attrs.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
    let src = get("src").unwrap_or("").trim();
    // A sanitized-away or missing source leaves nothing to draw.
    if src.is_empty() { return; }
    let lower = src.to_ascii_lowercase();
    let remote = lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("//");
    if remote && (pixel_sized(get("width"), get("height"), get("style")) || TRACKER_MARKERS.iter().any(|m| lower.contains(m))) {
        out.trackers_removed += 1;
        return;
    }
    if remote { out.remote_images += 1; }
    let rewritten = lower.starts_with("cid:").then(|| format!("cid://{}/{}", escape(&message.0), content_id(&src[4..])));
    out.html.push_str("<img");
    for (name, value) in &attrs {
        let value = if *name == "src" { rewritten.as_deref().unwrap_or(value) } else { value };
        out.html.push_str(&format!(" {name}=\"{value}\""));
    }
    out.html.push('>');
}

/// Both dimensions at most one pixel, from attributes or inline style.
fn pixel_sized(width: Option<&str>, height: Option<&str>, style: Option<&str>) -> bool {
    let tiny = |attr: Option<&str>, property: &str| {
        let from_style = style.and_then(|s| {
            s.to_ascii_lowercase().split(';').find_map(|d| d.trim().strip_prefix(property).and_then(|v| v.trim_start().strip_prefix(':')).map(|v| v.trim().to_owned()))
        });
        attr.map(str::to_owned).or(from_style).and_then(|v| v.trim_end_matches("px").trim().parse::<f32>().ok()).is_some_and(|n| n <= 1.0)
    };
    tiny(width, "width") && tiny(height, "height")
}

fn escape(s: &str) -> String { s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;") }

/// `cid:` targets are sometimes written with the header's angle brackets (escaped by now).
fn content_id(target: &str) -> &str {
    let t = target.trim();
    let t = t.strip_prefix("&lt;").unwrap_or(t);
    t.strip_suffix("&gt;").unwrap_or(t)
}

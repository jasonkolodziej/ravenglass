//! Stage 4: the allow-list sanitizer. `ammonia` removes scripts, frames, forms, handlers and
//! unknown URL schemes and writes normalized HTML; a second pass over that output filters CSS,
//! rewrites `cid:` images, drops tracking pixels and counts the remote images it left in place.
//! Remote images are not fetched here or anywhere by default: the renderer's net provider
//! refuses them until the user allows them.

use crate::css::filter_css;
use mail_core::MessageId;
use std::borrow::Cow;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    // `cellpadding` of each open table, innermost last (stage 3: the renderer ignores the attribute).
    let mut tables: Vec<Option<f32>> = Vec::new();
    while let Some(lt) = rest.find('<') {
        out.html.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let end = tag_end(rest);
        let (tag, after) = rest.split_at(end);
        if tag.starts_with("<table") {
            tables.push(attributes(tag).iter().find(|(n, _)| *n == "cellpadding").and_then(|(_, v)| v.trim().parse().ok()));
        } else if tag.starts_with("</table") {
            tables.pop();
        } else if (tag.starts_with("<td") || tag.starts_with("<th")) && !tag.starts_with("<thead")
            && let Some(Some(padding)) = tables.last()
        {
            out.html.push_str(&padded_cell(tag, *padding));
            rest = after;
            continue;
        }
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

/// A table cell with its table's `cellpadding` as CSS. The cell's own `padding` still wins,
/// because it comes later in the declaration list.
fn padded_cell(tag: &str, padding: f32) -> String {
    let attrs = attributes(tag);
    let name = if tag.starts_with("<td") { "td" } else { "th" };
    let mut out = format!("<{name}");
    let mut styled = false;
    for (attr, value) in &attrs {
        if *attr == "style" {
            styled = true;
            out.push_str(&format!(" style=\"padding:{padding}px;{value}\""));
        } else {
            out.push_str(&format!(" {attr}=\"{value}\""));
        }
    }
    if !styled { out.push_str(&format!(" style=\"padding:{padding}px\"")); }
    out.push('>');
    out
}

/// Replace every remote image with a box of the same size showing its `alt` text. The reader
/// uses this while images are blocked, so the layout holds and the reader sees what is missing.
/// `html` must be [`sanitize`] output.
pub fn placeholders(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let (tag, after) = rest.split_at(tag_end(rest));
        rest = after;
        if tag.starts_with("<style") {
            let close = rest.find("</style>").unwrap_or(rest.len());
            out.push_str(tag);
            out.push_str(&rest[..close]);
            rest = &rest[close..];
            continue;
        }
        let attrs = if tag.starts_with("<img") { attributes(tag) } else { Vec::new() };
        let get = |name: &str| attrs.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
        let src = get("src").unwrap_or("").trim().to_ascii_lowercase();
        if !(src.starts_with("http://") || src.starts_with("https://") || src.starts_with("//")) {
            out.push_str(tag);
            continue;
        }
        let size = |name: &str| get(name).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok());
        let mut style = String::from("display:inline-block;box-sizing:border-box;overflow:hidden;border:1px dashed #8a85ad;color:#6b6787;font-size:12px;padding:4px;max-width:100%;");
        if let Some(w) = size("width") { style.push_str(&format!("width:{w}px;")); }
        if let Some(h) = size("height") { style.push_str(&format!("height:{h}px;")); }
        out.push_str(&format!("<span style=\"{style}\">{}</span>", get("alt").unwrap_or("")));
    }
    out.push_str(rest);
    out
}

/// The remote image URLs in sanitized HTML, each once, in document order.
pub fn remote_images(html: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt..];
        let (tag, after) = rest.split_at(tag_end(rest));
        rest = after;
        if tag.starts_with("<style") { rest = rest.find("</style>").map_or("", |i| &rest[i..]); continue; }
        if !tag.starts_with("<img") { continue; }
        let Some((_, src)) = attributes(tag).into_iter().find(|(n, _)| *n == "src") else { continue };
        let url = src.trim().replace("&amp;", "&").replace("&quot;", "\"");
        let lower = url.to_ascii_lowercase();
        if (lower.starts_with("http://") || lower.starts_with("https://")) && !urls.contains(&url) { urls.push(url); }
    }
    urls
}

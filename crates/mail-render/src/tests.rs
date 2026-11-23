use super::*;

const PLAIN: &[u8] = include_bytes!("../../../fixtures/emails/01-plain-reply.eml");
const NEWSLETTER: &[u8] = include_bytes!("../../../fixtures/emails/02-newsletter-tables.eml");
const MULTIPART: &[u8] = include_bytes!("../../../fixtures/emails/03-multipart-cid.eml");

fn id() -> MessageId { MessageId("acc/1".into()) }

fn clean(html: &str) -> SanitizedHtml { sanitize(html, &id()) }

#[test]
fn plain_reply_parses_folds_quotes_and_finds_links() {
    let p = parse(PLAIN).unwrap();
    assert!(p.html.is_none() && p.from.is_some() && !p.subject.is_empty());
    let Body::Text(body) = p.body(&id(), Prefer::Html) else { panic!("plain mail has no HTML view") };
    assert!(body.lines.iter().any(|l| l.quote_depth > 0));
    assert!(body.links().next().is_some());
    let quoted = body.lines.iter().filter(|l| l.quote_depth > 0).count();
    assert_eq!(body.folded_lines(), quoted.saturating_sub(QUOTE_FOLD_AFTER));
}

#[test]
fn newsletter_keeps_layout_blocks_remote_content_and_drops_the_pixel() {
    let p = parse(NEWSLETTER).unwrap();
    let raw = p.html.as_deref().unwrap();
    assert!(raw.contains("bgcolor"));
    let Body::Html { html, text } = p.body(&id(), Prefer::Html) else { panic!("expected the HTML view") };
    assert!(html.html.contains("<table") && html.html.contains("bgcolor"));
    assert_eq!((html.remote_images, html.trackers_removed), (1, 1));
    assert!(!text.lines.is_empty(), "the text view exists for HTML-only mail");
    assert!(matches!(p.body(&id(), Prefer::Plain), Body::Text(_)));
    assert!(!p.index_text(&id()).contains('<'));
}

#[test]
fn multipart_exposes_the_inline_image_and_lists_only_the_real_attachment() {
    let p = parse(MULTIPART).unwrap();
    assert!(p.text.is_some() && p.html.is_some());
    assert_eq!(p.inline.len(), 1);
    assert_eq!(p.attachments.iter().map(|a| a.mime.as_str()).collect::<Vec<_>>(), ["application/pdf"]);
    let Body::Html { html, .. } = p.body(&id(), Prefer::Html) else { panic!("expected the HTML view") };
    assert!(html.html.contains(&format!("cid://acc/1/{}", p.inline[0].content_id)), "{}", html.html);
    assert_eq!(html.remote_images, 0);
}

#[test]
fn active_content_is_removed() {
    let dirty = r#"<p onclick="x()">hi<script>alert(1)</script><iframe src="https://e.example"></iframe>
        <form action="/x"><input name="a"></form><object data="x"></object><embed src="x">
        <a href="javascript:alert(1)">a</a><a href="https://ok.example/">b</a>
        <base href="https://evil.example/"><meta http-equiv="refresh" content="0;url=https://evil.example"></p>"#;
    let out = clean(dirty).html.to_ascii_lowercase();
    for gone in ["script", "alert", "onclick", "iframe", "<form", "<input", "<object", "<embed", "javascript:", "<base", "<meta", "evil.example"] {
        assert!(!out.contains(gone), "`{gone}` survived in {out}");
    }
    assert!(out.contains("https://ok.example/"));
}

#[test]
fn css_keeps_styling_but_cannot_fetch_or_escape() {
    let dirty = r#"<style>@import url("https://e.example/a.css"); .a{color:red;background:url(https://e.example/b.png)}
        .b{position : fixed; background:u\72l(https://e.example/c.png)} .c{background:ur/**/l(https://e.example/d.png)}</style>
        <div style="color:#123456;background-image:URL('https://e.example/e.png');position:fixed">x</div>
        <div style="background:url(cid:logo)">y</div>"#;
    let out = clean(dirty).html;
    assert!(!out.contains("e.example") && !out.contains("@import") && !out.to_ascii_lowercase().contains("fixed"), "{out}");
    assert!(out.contains("color:red") && out.contains("color:#123456") && out.contains("url(cid:logo)"), "{out}");
}

#[test]
fn images_data_urls_and_trackers() {
    let out = clean(concat!(
        r#"<img src="https://cdn.example/hero.png" width="600" height="200">"#,
        r#"<img src="https://t.example/p?id=1" width="1" height="1">"#,
        r#"<img src="https://t.example/p" style="width:1px;height:1px">"#,
        r#"<img src="https://mail.example/wf/open?u=1" width="20" height="20">"#,
        r#"<img src="data:image/png;base64,AAAA"><img src="data:text/html;base64,AAAA"><img src="data:image/svg+xml,x">"#,
        r#"<a href="data:text/html,x">l</a><img src="CID:&lt;part1@x&gt;" alt="a &quot;b&quot; > c">"#,
    ));
    assert_eq!((out.remote_images, out.trackers_removed), (1, 3));
    assert!(out.html.contains("data:image/png") && !out.html.contains("data:text") && !out.html.contains("svg"), "{}", out.html);
    assert!(out.html.contains(r#"src="cid://acc/1/part1@x""#), "{}", out.html);
    assert!(out.html.contains("alt=\"a &quot;b&quot; &gt; c\""), "{}", out.html);
    assert!(!out.html.contains("<img>"), "{}", out.html);
}

#[test]
fn html_to_text_keeps_reading_order_links_lists_and_quotes() {
    let out = clean(r#"<style>p{color:red}</style><h1>Title</h1><p>One &amp; two<br>three <a href="https://x.example/a">here</a></p>
        <ul><li>first</li><li>second</li></ul><blockquote><p>quoted</p></blockquote><table><tr><td>a</td><td>b</td></tr></table>
        <a href="https://same.example/">https://same.example/</a><img src="cid:x" alt="Logo">&#169;"#);
    let text = html_to_text(&out.html);
    assert_eq!(text, "Title\n\nOne & two\nthree here (https://x.example/a)\n\n• first\n• second\n\n> quoted\n\na b\n\nhttps://same.example/[Logo]©");
}

#[test]
fn text_view_details() {
    let body = TextBody::from_plain("See https://a.example/x, then (http://b.example). mailto: is not a link\n> q1\n>> q2\n> q3\n> q4\n> q5\nafter\n\n");
    assert_eq!(body.links().collect::<Vec<_>>(), ["https://a.example/x", "http://b.example"]);
    assert_eq!(body.lines.iter().map(|l| l.quote_depth).collect::<Vec<_>>(), [0, 1, 2, 1, 1, 1, 0]);
    assert_eq!(body.lines.iter().map(|l| l.folded).collect::<Vec<_>>(), [false, false, false, false, true, true, false]);
    assert_eq!(body.lines[0].text(), "See https://a.example/x, then (http://b.example). mailto: is not a link");
}

#[test]
fn hostile_and_broken_input_never_panics() {
    assert_eq!(parse(&vec![b'a'; MAX_MESSAGE_BYTES + 1]), Err(RenderError::TooLarge));
    for html in ["<", "<img", "<img src=\"", "<style>", "<style>url(", "<a href=\"x", "&#xFFFFFFFF;&#;&", "<img src=cid:>", "<table><td><style>@import", "\u{0}<p>é<img alt=\"é\">"] {
        let out = clean(html);
        html_to_text(&out.html);
    }
    for raw in [&b""[..], b"\r\n\r\n", b"Content-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\n", b"\xff\xfe\x00"] {
        if let Ok(p) = parse(raw) { p.body(&id(), Prefer::Html); }
    }
}

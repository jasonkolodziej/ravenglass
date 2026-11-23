use super::*;
use mail_core::MessageId;

const NEWSLETTER: &[u8] = include_bytes!("../../../fixtures/emails/02-newsletter-tables.eml");
const MULTIPART: &[u8] = include_bytes!("../../../fixtures/emails/03-multipart-cid.eml");

fn view(raw: &[u8], images: bool, options: Options) -> HtmlView {
    let parsed = mail_render::parse(raw).unwrap();
    let clean = mail_render::sanitize(parsed.html.as_deref().unwrap(), &MessageId::from("acc/1"));
    let images = if images { Images { inline: parsed.inline, ..Default::default() } } else { Images::default() };
    HtmlView::new(&clean.html, images, options).unwrap()
}

/// Share of pixels in a tile that are close to `rgb`.
fn share(tile: &Tile, rgb: [u8; 3]) -> f32 {
    let near = tile.rgba.as_chunks::<4>().0.iter().filter(|p| p[..3].iter().zip(rgb).all(|(a, b)| a.abs_diff(b) < 12)).count();
    near as f32 / (tile.width * tile.height) as f32
}

#[test]
fn newsletter_lays_out_and_paints_its_own_colours() {
    let mut v = view(NEWSLETTER, false, Options::default());
    assert_eq!(v.size().0, 680);
    assert!(v.height() > 300.0, "{}", v.height());
    let tile = v.tile(0).unwrap();
    assert_eq!(tile.rgba.len(), (tile.width * tile.height * 4) as usize);
    assert!(share(&tile, [0xff, 0xff, 0xff]) > 0.4, "the white card");
    assert!(share(&tile, [0xee, 0xf0, 0xf4]) > 0.05, "the grey page behind it");
    assert!(share(&tile, [0x5b, 0x4f, 0xd6]) > 0.0005, "the button");
    assert!(v.tile(v.tile_count()).is_none());
}

#[test]
fn scale_and_width_change_the_bitmap_not_the_content() {
    let text = format!("<p>{}</p>", "word ".repeat(300));
    let mut v = HtmlView::new(&text, Images::default(), Options { scale: 2.0, ..Default::default() }).unwrap();
    assert_eq!(v.size().0, 1360);
    let at_680 = v.height();
    v.set_width(400);
    assert_eq!(v.size().0, 800);
    assert!(v.height() > at_680, "narrower text wraps onto more lines: {} vs {at_680}", v.height());
    assert_eq!(v.tile(0).unwrap().width, 800);
    v.set_width(680);
    assert_eq!(v.height(), at_680);
}

#[test]
fn tall_documents_come_in_tiles_that_join_without_gaps() {
    let body: String = (0..400).map(|i| format!("<p style=\"background:{}\">row {i}</p>", if i % 2 == 0 { "#ff0000" } else { "#0000ff" })).collect();
    let mut v = HtmlView::new(&body, Images::default(), Options::default()).unwrap();
    assert!(v.tile_count() >= 3, "{}", v.tile_count());
    let (width, total) = v.size();
    let tiles: Vec<Tile> = (0..v.tile_count()).map(|i| v.tile(i).unwrap()).collect();
    assert_eq!(tiles.iter().map(|t| t.height).sum::<u32>(), total);
    assert!(tiles.iter().all(|t| t.width == width && t.y == t.index * TILE_H));
    // A later tile shows later rows, not the top again.
    assert_ne!(tiles[0].rgba[..4096], tiles[1].rgba[..4096]);
    let mut whole = HtmlView::new(&body, Images::default(), Options::default()).unwrap();
    assert_eq!(whole.tile(1).unwrap(), tiles[1], "tiles are independent of paint order");
}

#[test]
fn only_the_messages_own_images_load() {
    let with = view(MULTIPART, true, Options::default());
    let without = view(MULTIPART, false, Options::default());
    assert_eq!((with.refused_requests(), without.refused_requests()), (0, 1), "the cid image is served only when the part is supplied");
    // Remote images are replaced by placeholders, so nothing is even requested.
    assert_eq!(view(NEWSLETTER, false, Options::default()).refused_requests(), 0);
    // If a remote URL does reach the document, it is refused.
    let leaky = HtmlView::new(r#"<img src="https://t.example/p.png"><link rel="stylesheet" href="https://t.example/a.css"><p>x</p>"#, Images { remote: [("https://other.example/ok.png".to_owned(), vec![0])].into(), ..Default::default() }, Options::default()).unwrap();
    assert!(leaky.refused_requests() >= 1);
}

#[test]
fn links_are_found_under_the_pointer_and_suspicious_ones_are_flagged() {
    let v = view(NEWSLETTER, false, Options::default());
    let mut found = Vec::new();
    for y in (4..v.height() as u32).step_by(6) {
        for x in (4..680).step_by(10) {
            if let Some(link) = v.link_at(x as f32, y as f32) && !found.contains(&link) { found.push(link); }
        }
    }
    assert_eq!(found.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Read the notes", "Unsubscribe"]);
    assert_eq!(found[0].href, "https://blitz.example/release/0.3");
    assert!(v.link_at(2.0, 2.0).is_none());
    let link = |text: &str, href: &str| Link { text: text.into(), href: href.into() };
    assert!(link("paypal.com", "https://paypa1.example/login").text_names_another_host());
    assert!(!link("https://www.example.com/a", "http://example.com/b?x").text_names_another_host());
    assert!(!link("Read the notes", "https://blitz.example/").text_names_another_host());
}

#[test]
fn darken_turns_the_paper_dark_and_keeps_hues() {
    let mut paper = view(NEWSLETTER, false, Options::default());
    let mut dark = view(NEWSLETTER, false, Options { dark: DarkMode::Darken, ..Default::default() });
    let (p, d) = (paper.tile(0).unwrap(), dark.tile(0).unwrap());
    assert!(share(&p, [0xff, 0xff, 0xff]) > 0.4 && share(&d, [0, 0, 0]) > 0.4);
    let mut px = [0x5b, 0x4f, 0xd6, 0xff, 255, 255, 255, 255, 10, 10, 10, 255];
    darken(&mut px);
    assert!(px[2] > px[0] && px[0] > px[1], "the violet button stays violet: {px:?}");
    assert_eq!(px[4..7], [0, 0, 0]);
    assert_eq!(px[8..11], [245, 245, 245]);
}

#[test]
fn panics_and_empty_documents_become_failures() {
    assert_eq!(guarded::<()>(|| panic!("inside the renderer")), Err(Failure::Crashed));
    assert_eq!(guarded(|| Ok(7)), Ok(7));
    for html in ["", "<", "<table><tr><td><table>", "<style>*{display:none}</style><p>x</p>", "<p style=\"height:99999999px\">x</p>"] {
        if let Ok(mut v) = guarded(|| HtmlView::new(html, Images::default(), Options::default())) {
            assert!(v.is_renderable());
            let _ = guarded(|| Ok(v.tile(0)));
        }
    }
    let hostile = Options { paper: "red;} body{background:url(https://t.example/x)".into(), ..Default::default() };
    assert_eq!(HtmlView::new("<p>x</p>", Images::default(), hostile).unwrap().refused_requests(), 0);
}

#[test]
fn renderer_thread_serves_views_by_id_and_survives_a_bad_one() {
    let renderer = Renderer::spawn().unwrap();
    let parsed = mail_render::parse(NEWSLETTER).unwrap();
    let clean = mail_render::sanitize(parsed.html.as_deref().unwrap(), &MessageId::from("acc/1"));
    let info = renderer.open(1, clean.html.clone(), Images::default(), Options::default()).unwrap();
    assert_eq!((info.width, info.tiles), (680, 1));
    assert_eq!(renderer.tile(1, 0).unwrap().unwrap().height, info.height);
    assert_eq!(renderer.tile(1, 9).unwrap(), None);
    let narrow = renderer.set_width(1, 300).unwrap();
    assert_eq!(narrow.width, 300);
    assert!(renderer.open(2, String::new(), Images::default(), Options::default()).is_ok(), "an empty message is a small blank card");
    assert_eq!(renderer.tile(7, 0), Err(Failure::Closed), "an unknown view");
    assert!(renderer.link_at(1, 2.0, 2.0).unwrap().is_none());
    renderer.close(1);
    assert_eq!(renderer.tile(1, 0), Err(Failure::Closed));
    // The handle is what crosses threads; the documents never leave the render thread.
    std::thread::spawn(move || assert!(renderer.open(3, "<p>x</p>".into(), Images::default(), Options::default()).is_ok())).join().unwrap();
}

/// Every fixture email goes through the whole pipeline: parse, sanitize, lay out, paint every
/// tile, hit-test. Nothing may panic, fetch, or run past the budget (architecture §7).
#[test]
fn the_fixture_corpus_renders_within_budget_and_fetches_nothing() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/emails");
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "eml")).collect();
    files.sort();
    assert!(files.len() >= 11, "the corpus is missing files");
    let mut rendered = 0;
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let parsed = mail_render::parse(&std::fs::read(&path).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!parsed.subject.is_empty(), "{name}");
        let Some(raw) = &parsed.html else { continue };
        let clean = mail_render::sanitize(raw, &MessageId::from(name.as_str()));
        for banned in ["<script", "<form", "<input", "@import", "javascript:", "position:fixed", "position: fixed"] {
            assert!(!clean.html.to_ascii_lowercase().contains(banned), "{name}: `{banned}` survived sanitizing");
        }
        let started = std::time::Instant::now();
        let opened = guarded(|| HtmlView::new(&clean.html, Images { inline: parsed.inline.clone(), ..Default::default() }, Options { scale: 2.0, ..Default::default() }));
        // The malformed fixture asks for an absurd height, so it must be refused, not painted.
        if name.starts_with("09-") { assert_eq!(opened.err(), Some(Failure::TooLarge), "{name}"); continue; }
        let mut view = opened.unwrap_or_else(|e| panic!("{name}: {e}"));
        let first = view.tile(0).unwrap_or_else(|| panic!("{name}: no first tile"));
        let first_paint = started.elapsed();
        for index in 1..view.tile_count() { assert!(view.tile(index).is_some(), "{name}: tile {index}"); }
        for y in (0..view.height() as u32).step_by(40) { view.link_at(100.0, y as f32); }
        assert_eq!(view.refused_requests(), 0, "{name}: a request reached the network layer");
        assert!(first.rgba.iter().any(|b| *b != first.rgba[0]), "{name}: the first tile is blank");
        eprintln!("{name}: {}x{} px, {} tiles, first paint {first_paint:?}", view.size().0, view.size().1, view.tile_count());
        // 250 ms is the release budget for a heavy newsletter; debug builds get 20x.
        assert!(first_paint.as_millis() < 5000, "{name}: first paint took {first_paint:?}");
        rendered += 1;
    }
    assert!(rendered >= 9);
}

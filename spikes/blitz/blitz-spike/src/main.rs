//! M1 spike (plan §3.5): fixture emails → mail-render sanitizer → Blitz layout → CPU raster.
//! Headless: writes one PNG per fixture and scale, prints timings and the links hit-testing finds.
//!
//!   cargo run --release -- ../../../fixtures/emails out
//!
//! With `SPIKE_EML=<file>` it instead opens a Rune window showing that email through
//! `rune::Pixels` (Rune's own flags apply, e.g. `-- --snap out/window.png`).

use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{BaseDocument, DocumentConfig, local_name};
use blitz_html::HtmlDocument;
use blitz_traits::net::DummyNetProvider;
use blitz_traits::shell::{ColorScheme, Viewport};
use mail_core::MessageId;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WIDTH: u32 = 680;
/// The email sits on a light "paper" card whatever the app theme is.
const UA_CSS: &str = "html{background:#fff;color:#17142a} body{margin:16px;font-family:sans-serif;word-break:break-word} img,table{max-width:100%} a{color:#1f6fbf}";

fn document(html: &str, scale: f32) -> BaseDocument {
    let config = DocumentConfig {
        viewport: Some(Viewport::new((WIDTH as f32 * scale) as u32, (800.0 * scale) as u32, scale, ColorScheme::Light)),
        // Deny by default: nothing remote is ever requested.
        net_provider: Some(Arc::new(DummyNetProvider)),
        ..Default::default()
    };
    let mut doc = HtmlDocument::from_html(&format!("<style>{UA_CSS}</style>{html}"), config).into_inner();
    doc.resolve(0.0);
    doc
}

fn height(doc: &BaseDocument) -> f32 { doc.root_element().final_layout().size.height }

fn raster(doc: &mut BaseDocument, scale: f32) -> (u32, u32, Vec<u8>) {
    let (w, h) = ((WIDTH as f32 * scale) as u32, ((height(doc) * scale).ceil() as u32).clamp(1, u16::MAX as u32));
    let mut renderer = VelloCpuImageRenderer::new(w, h);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    renderer.render_to_vec(|scene| blitz_paint::paint_scene(scene, doc, scale as f64, w, h, 0, 0), &mut rgba);
    (w, h, rgba)
}

/// Links found by probing the laid-out document on a coarse grid, as pointer hover would.
fn links(doc: &BaseDocument) -> Vec<String> {
    let mut found = Vec::new();
    let mut y = 4.0;
    while y < height(doc) {
        for x in (4..WIDTH).step_by(12) {
            let Some(hit) = doc.hit(x as f32, y) else { continue };
            let mut node = doc.get_node(hit.node_id);
            while let Some(n) = node {
                if let Some(href) = n.element_data().filter(|e| e.name.local == local_name!("a")).and_then(|e| e.attr(local_name!("href"))) {
                    if !found.iter().any(|f| f == href) { found.push(href.to_owned()); }
                    break;
                }
                node = n.parent.and_then(|p| doc.get_node(p));
            }
        }
        y += 8.0;
    }
    found
}

fn median(mut times: Vec<Duration>) -> Duration { times.sort(); times[times.len() / 2] }

struct Shown { pixels: rune::Pixels, size: (f32, f32) }

/// The Rune side of the bridge: the bitmap in a `Pixels` element on the app's background.
fn view(s: &Shown, _: &mut rune::Cx<Shown>) -> rune::El {
    let t = rune::tok::theme(true);
    rune::col().w_full().h_full().center().fill(t.bg)
        .child(rune::el().key("email").size(s.size.0, s.size.1).radius(6.0).pixels(&s.pixels))
}

/// `AppSpec` takes a plain `fn`, so the file comes from the environment, not a capture.
fn shown() -> Shown {
    let path = std::env::var("SPIKE_EML").expect("SPIKE_EML");
    let parsed = mail_render::parse(&std::fs::read(path).unwrap()).unwrap();
    let clean = mail_render::sanitize(parsed.html.as_deref().expect("an HTML email"), &MessageId::from("spike"));
    let scale = 2.0;
    let mut doc = document(&clean.html, scale);
    let (w, h, rgba) = raster(&mut doc, scale);
    let pixels = rune::Pixels::default();
    pixels.set(w, h, rgba);
    Shown { pixels, size: (w as f32 / scale, h as f32 / scale) }
}

fn main() {
    if std::env::var("SPIKE_EML").is_ok() { return rune::run(rune::AppSpec::<Shown>::new("Blitz spike", (760.0, 480.0), shown, view)); }
    let mut args = std::env::args().skip(1);
    let (dir, out) = (args.next().expect("fixtures dir"), args.next().unwrap_or_else(|| "out".into()));
    std::fs::create_dir_all(&out).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "eml")).collect();
    files.sort();
    for path in files {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let parsed = mail_render::parse(&std::fs::read(&path).unwrap()).unwrap();
        let Some(raw_html) = &parsed.html else { println!("{name}: plain text, text view only"); continue };
        let clean = mail_render::sanitize(raw_html, &MessageId::from(name.as_str()));
        for scale in [1.0f32, 2.0] {
            let (mut layout, mut paint, mut last) = (Vec::new(), Vec::new(), None);
            for _ in 0..5 {
                let t = Instant::now();
                let mut doc = document(&clean.html, scale);
                layout.push(t.elapsed());
                let t = Instant::now();
                last = Some(raster(&mut doc, scale));
                paint.push(t.elapsed());
            }
            let (w, h, rgba) = last.unwrap();
            let file = std::fs::File::create(format!("{out}/{name}@{scale}x.png")).unwrap();
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
            println!("{name} @{scale}x: {w}x{h} px, parse+style+layout {:?}, raster {:?}, {} MB RGBA", median(layout), median(paint), rgba.len() >> 20);
        }
        println!("{name}: links by hit-testing: {:?}", links(&document(&clean.html, 1.0)));
    }
}

use crate::grid::{self, Grid};
use crate::timefmt;
use crate::view::reader::wrap;
use mail_render::Span;

#[test]
fn cells_snap_to_device_pixels_and_tracking_closes_the_gap() {
    // IBM Plex Mono advances 0.6 em: 8.4 pt at 14 pt.
    let retina = Grid::from_metrics(14.0, 1.35, 8.4, 2.0);
    assert_eq!(retina.cell_w, 8.5);
    assert_eq!(retina.cell_h, 19.0);
    assert!((retina.tracking - 0.1).abs() < 1e-4);
    let plain = Grid::from_metrics(14.0, 1.35, 8.4, 1.0);
    assert_eq!((plain.cell_w, plain.cell_h), (8.0, 19.0));
    assert!((plain.tracking + 0.4).abs() < 1e-4);
    for g in [retina, plain] {
        assert!((8.4 + g.tracking - g.cell_w).abs() < 1e-4, "advance plus tracking is one cell");
    }
}

#[test]
fn a_zero_scale_is_treated_as_one() {
    assert_eq!(Grid::from_metrics(14.0, 1.35, 8.4, 0.0), Grid::from_metrics(14.0, 1.35, 8.4, 1.0));
}

#[test]
fn the_real_font_is_monospace_at_six_tenths_of_an_em() {
    // The measurer uses the fonts of the most recent text system: give it the app's.
    let _text = rune::text::TextSystem::new(crate::FONTS, crate::FAMILIES);
    let g = Grid::measure(14.0, 1.35, 2.0);
    assert_eq!(g.cell_w, 8.5, "IBM Plex Mono's 0 is 8.4 pt wide at 14 pt");
    let style = g.style(400.0);
    let narrow = rune::text::advance("iiiiiiiiii", style);
    let wide = rune::text::advance("MMMMMMMMMM", style);
    assert!((narrow - wide).abs() < 0.01, "every glyph advances the same: {narrow} vs {wide}");
    assert!((wide - g.x(10)).abs() < 0.05, "ten characters are ten cells: {wide} vs {}", g.x(10));
}

#[test]
fn whole_cells_fit_in_a_width() {
    let g = Grid::from_metrics(14.0, 1.35, 8.4, 2.0);
    assert_eq!(g.cols_in(1400.0), 164);
    assert_eq!(g.rows_in(880.0), 46);
    assert_eq!(g.cols_in(g.x(83)), 83, "a whole number of cells survives the round trip");
    assert_eq!(g.rows_in(g.y(42)), 42);
}

#[test]
fn text_is_cut_and_padded_in_cells() {
    assert_eq!(grid::fit("Odin", 8), "Odin    ");
    assert_eq!(grid::fit("Odin Allfather", 8), "Odin Al…");
    assert_eq!(grid::fit_right("12", 5), "   12");
    assert_eq!(grid::fit_right("123456", 4), "123…");
    assert_eq!(grid::clip("naïve café", 6), "naïve…");
    assert_eq!(grid::clip("abc", 0), "");
    for n in 0..12 {
        assert_eq!(grid::width(&grid::fit("Kolodziej Lab — week 46", n)), n);
        assert_eq!(grid::width(&grid::fit_right("Kolodziej Lab — week 46", n)), n);
    }
}

#[test]
fn dates_read_as_the_grid_shows_them() {
    let now = mail_mock::NOW;
    assert_eq!(timefmt::long(now), "Fri 13 Nov 2026 16:44");
    assert_eq!(timefmt::short(now - 3_600, now), "15:44");
    assert_eq!(timefmt::short(now - 86_400, now), "Thu");
    assert_eq!(timefmt::short(now - 20 * 86_400, now), "24 Oct");
    assert_eq!(timefmt::short(now - 400 * 86_400, now), "Oct 25");
    assert!(grid::width(&timefmt::short(now - 400 * 86_400, now)) <= 6);
    assert_eq!(timefmt::bytes(2_100_000), "2.1 MB");
    assert_eq!(timefmt::bytes(840_000), "840 KB");
}

fn text(rows: &[Vec<(String, bool)>]) -> Vec<String> {
    rows.iter().map(|r| r.iter().map(|(t, _)| t.as_str()).collect()).collect()
}

#[test]
fn body_lines_wrap_at_spaces_within_the_width() {
    let spans = [Span::Text("Huginn reports the northern pass is clear and the road is open".into())];
    let rows = wrap(&spans, 24);
    assert_eq!(text(&rows), ["Huginn reports the", "northern pass is clear", "and the road is open"]);
    assert!(text(&rows).iter().all(|r| grid::width(r) <= 24));
}

#[test]
fn links_stay_whole_and_marked_when_they_fit() {
    let spans = [Span::Text("The schedule is here: ".into()), Span::Link("https://ferries.example/x".into()), Span::Text(" ok".into())];
    let rows = wrap(&spans, 30);
    assert_eq!(text(&rows), ["The schedule is here:", "https://ferries.example/x ok"]);
    assert_eq!(rows[1][0], ("https://ferries.example/x".to_owned(), true));
}

#[test]
fn an_unbreakable_run_is_cut_at_the_width() {
    let rows = wrap(&[Span::Text("a".repeat(25))], 10);
    assert_eq!(text(&rows), ["a".repeat(10), "a".repeat(10), "a".repeat(5)]);
    assert_eq!(text(&wrap(&[], 10)), [""]);
}

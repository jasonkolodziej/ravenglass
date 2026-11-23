use super::*;

#[test]
fn builtins_parse_and_resolve_every_role() {
    for t in [Theme::night(), Theme::frost()] {
        assert_eq!(t.palette.len(), NAMES.len(), "{} defines all names", t.name);
        for role in t.roles.keys() { assert_ne!(t.role(role), Rgb::MAGENTA, "{role}"); }
    }
}

#[test]
fn builtins_pass_contrast() {
    for t in [Theme::night(), Theme::frost()] {
        assert!(t.contrast_report(4.5).is_empty(), "{}: {:?}", t.name, t.contrast_report(4.5));
    }
}

#[test]
fn a_two_line_theme_is_valid() {
    let t = Theme::parse("base = \"ravenglass-night\"\npalette.aqua = \"#FF9F43\"", &[]).unwrap();
    assert_eq!(t.role("mail.unread").hex(), "#FF9F43");
    assert_eq!(t.color("background"), Theme::night().color("background"));
}

#[test]
fn unknown_names_and_keys_are_rejected() {
    assert!(matches!(Theme::parse("palette.chartreuse = \"#00ff00\"", &[]), Err(ThemeError::UnknownName(_))));
    assert!(Theme::parse("script = \"rm -rf /\"", &[]).is_err(), "unknown top-level keys are rejected");
    assert!(matches!(Theme::parse("roles.\"mail.unread\" = \"nope\"", &[]), Err(ThemeError::BadRole { .. })));
}

#[test]
fn fix_contrast_always_passes() {
    let bgs = ["#0E0C1A", "#F4F5FB", "#808080", "#FFFFFF", "#000000"];
    let fgs = ["#1A1A1A", "#777777", "#A9DCFF", "#FF0000", "#EEEEEE", "#2F2A80"];
    for bg in bgs { for fg in fgs {
        let (bg, fg) = (Rgb::parse(bg).unwrap(), Rgb::parse(fg).unwrap());
        let fixed = fix_contrast(fg, bg, 4.5);
        assert!(contrast(fixed, bg) >= 4.5, "{} on {} → {}", fg.hex(), bg.hex(), fixed.hex());
    }}
}

#[test]
fn ghostty_import_round_trips() {
    let src = "background = 1e1e2e\nforeground = cdd6f4\ncursor-color = f5e0dc\npalette = 1=#f38ba8\npalette = 14=#94e2d5\n";
    let toml = import_ghostty(src, "Mocha", "ravenglass-night").unwrap();
    let t = Theme::parse(&toml, &[]).unwrap();
    assert_eq!(t.color("background").hex(), "#1E1E2E");
    assert_eq!(t.color("aqua").hex(), "#94E2D5");
    assert_eq!(t.color("maroon").hex(), "#F38BA8");
}

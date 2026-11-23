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

/// Each importer, and the format detection in `import`, on the same three colours.
#[test]
fn every_terminal_format_imports_to_the_same_theme() {
    let kitty = "# Mocha\nbackground #1e1e2e\nforeground #cdd6f4\ncursor none\ncolor1 #f38ba8\ncolor14 #94e2d5\n";
    let alacritty = "[colors.primary]\nbackground = '#1e1e2e'\nforeground = '0xcdd6f4'\n[colors.cursor]\ncursor = 'CellForeground'\n[colors.normal]\nred = '#f38ba8'\n[colors.bright]\ncyan = '#94e2d5'\n";
    let windows = r##"{"name":"Mocha","background":"#1E1E2E","foreground":"#CDD6F4","red":"#F38BA8","brightCyan":"#94E2D5"}"##;
    let component = |hex: &str| { let v = u8::from_str_radix(hex, 16).unwrap() as f32 / 255.0; format!("{v}") };
    let dict = |key: &str, rgb: &str| format!("<key>{key}</key><dict><key>Alpha Component</key><real>1</real><key>Blue Component</key><real>{}</real><key>Color Space</key><string>sRGB</string><key>Green Component</key><real>{}</real><key>Red Component</key><real>{}</real></dict>", component(&rgb[4..6]), component(&rgb[2..4]), component(&rgb[0..2]));
    let iterm = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>{}{}{}{}</dict></plist>", dict("Ansi 1 Color", "f38ba8"), dict("Ansi 14 Color", "94e2d5"), dict("Background Color", "1e1e2e"), dict("Foreground Color", "cdd6f4"));
    let ghostty = "background = 1e1e2e\nforeground = cdd6f4\npalette = 1=#f38ba8\npalette = 14=#94e2d5\n";
    for (format, src) in [("kitty", kitty), ("alacritty", alacritty), ("windows terminal", windows), ("iterm2", iterm.as_str()), ("ghostty", ghostty)] {
        let toml = import(src, "Mocha", "ravenglass-night").unwrap_or_else(|e| panic!("{format}: {e}"));
        let t = Theme::parse(&toml, &[]).unwrap_or_else(|e| panic!("{format}: {e}\n{toml}"));
        let got = [t.color("background").hex(), t.color("foreground").hex(), t.color("maroon").hex(), t.color("aqua").hex()];
        assert_eq!(got, ["#1E1E2E", "#CDD6F4", "#F38BA8", "#94E2D5"], "{format}");
        assert_eq!(t.color("cursor"), Theme::night().color("cursor"), "{format}: a keyword cursor keeps the base colour");
    }
}

#[test]
fn imports_reject_bad_colours_and_cannot_inject_theme_keys() {
    assert!(import_kitty("color1 #zzzzzz\n", "x", "ravenglass-night").is_err());
    assert!(import_windows_terminal(r#"{"red":"not a colour"}"#, "x", "ravenglass-night").is_err());
    assert!(import_iterm2("<plist><dict><key>Ansi 1 Color</key><dict><key>Red Component</key><real>x</real></dict></dict></plist>", "x", "ravenglass-night").is_err());
    let hostile = "evil\"\n[ui]\nwallpaper = \"/etc/passwd";
    let toml = import_ghostty("background = 000000\n", hostile, "ravenglass-night").unwrap();
    let t = Theme::parse(&toml, &[]).unwrap();
    assert_eq!(t.ui.wallpaper, Theme::night().ui.wallpaper, "the name stays a name");
    assert!(t.name.starts_with("evil\""));
}

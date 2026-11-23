use super::*;

/// The example in architecture appendix A, verbatim.
const APPENDIX_A: &str = r##"
[ui]
theme = "ravenglass-night"       # or "system" with [ui.system] light/dark
reader_default = "html"           # html | text
html_dark = "paper"               # paper | darken
undo_send_seconds = 10

[keys]
archive = "e"
delete = "#"
palette = "mod+shift+p"

[[workspace]]
slot = 1
source = "account:icloud/folder:INBOX"
layout = "vsplit(list, hsplit(reader, compose))"

[privacy]
remote_images = "ask"             # ask | never | always
"##;

#[test]
fn the_documented_example_parses() {
    let c = Config::parse(APPENDIX_A).unwrap();
    assert_eq!((c.ui.reader_default, c.ui.html_dark, c.ui.undo_send_seconds), (ReaderDefault::Html, HtmlDark::Paper, 10));
    assert_eq!(c.keys["delete"], "#");
    assert_eq!((c.workspaces[0].slot, c.workspaces[0].source.as_str()), (1, "account:icloud/folder:INBOX"));
    assert_eq!(c.privacy.remote_images, RemoteImages::Ask);
}

#[test]
fn an_empty_file_is_the_defaults_and_they_match_the_decisions() {
    let c = Config::parse("").unwrap();
    assert_eq!(c, Config::default());
    assert_eq!(c.ui.reader_default, ReaderDefault::Html, "HTML is the default reader view");
    assert_eq!(c.ui.html_dark, HtmlDark::Paper);
    assert_eq!((c.theme_for(true), c.theme_for(false)), ("ravenglass-night", "ravenglass-night"));
    let system = Config::parse("[ui]\ntheme = \"system\"\n[ui.system]\nlight = \"mine\"").unwrap();
    assert_eq!((system.theme_for(true), system.theme_for(false)), ("ravenglass-night", "mine"));
}

#[test]
fn mistakes_are_errors_that_name_the_problem() {
    for (src, needle) in [
        ("[ui]\ntheem = \"x\"", "theem"),
        ("[ui]\nreader_default = \"pdf\"", "pdf"),
        ("[ui]\nundo_send_seconds = 31", "30"),
        ("[[workspace]]\nslot = 0\nsource = \"account:a\"", "slot 0"),
        ("[[workspace]]\nslot = 2\nsource = \"account:a\"\n[[workspace]]\nslot = 2\nsource = \"search:x\"", "twice"),
        ("[[workspace]]\nslot = 3\nsource = \"inbox\"", "account:"),
        ("[keys]\narchive = \" \"", "archive"),
        ("[telemetry]\nenabled = true", "telemetry"),
    ] {
        let error = Config::parse(src).expect_err(src).to_string();
        assert!(error.contains(needle), "`{src}` gave `{error}`");
    }
}

#[test]
fn saving_and_loading_round_trips() {
    let mut c = Config::parse(APPENDIX_A).unwrap();
    c.ui.html_dark = HtmlDark::Darken;
    c.privacy.lock_after_minutes = 15;
    assert_eq!(Config::parse(&c.to_toml()).unwrap(), c);
}

#[test]
fn with_theme_changes_only_the_theme_and_keeps_comments() {
    let out = with_theme(APPENDIX_A, "ravenglass-frost").unwrap();
    assert!(out.contains(r#"theme = "ravenglass-frost"       # or "system" with [ui.system] light/dark"#), "{out}");
    assert_eq!(out.replace("ravenglass-frost", "ravenglass-night"), APPENDIX_A);
    assert_eq!(Config::parse(&out).unwrap().ui.theme, "ravenglass-frost");
}

#[test]
fn with_theme_adds_the_key_or_the_table_when_missing() {
    let empty = with_theme("", "system").unwrap();
    assert_eq!(Config::parse(&empty).unwrap().ui.theme, "system");
    let src = "# mine\n[ui]\nreader_default = \"text\"\n\n[privacy]\nlock_on_launch = true\n";
    let out = with_theme(src, "my-theme").unwrap();
    assert!(out.starts_with("# mine\n[ui]\nreader_default = \"text\"\n"), "{out}");
    let parsed = Config::parse(&out).unwrap();
    assert_eq!((parsed.ui.theme.as_str(), parsed.ui.reader_default, parsed.privacy.lock_on_launch), ("my-theme", ReaderDefault::Text, true));
}

#[test]
fn with_theme_refuses_a_file_that_does_not_parse() {
    assert!(with_theme("[ui]\nnot_a_key = 1\n", "x").is_err());
    assert!(with_theme("[ui\n", "x").is_err());
}

#[test]
fn with_theme_quotes_the_name() {
    let out = with_theme("", "a \"quoted\" name").unwrap();
    assert_eq!(Config::parse(&out).unwrap().ui.theme, "a \"quoted\" name");
}

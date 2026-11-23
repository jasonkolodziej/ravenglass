//! The profile on disk and the settings read from it: where it lives on each platform, what the
//! files hold (never a secret), and how `config.toml` changes the running app. Every test that
//! touches the disk does so in a folder of its own under the system's temporary directory.

use crate::keymap;
use crate::platform;
use crate::profile::{self, Loaded, Profile, parse_args};
use crate::shell::Services;
use crate::state::{AppState, Source};
use mail_auth::oauth::Provider;
use mail_auth::{AccountConfig, AuthKind, Incoming, MemorySecrets, Security, Server};
use mail_core::AccountId;
use ravenglass_config::Config;
use ravenglass_engine::{DarkMode, ReaderView};
use ravenglass_theme::Theme;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A profile folder of this test's own, removed when the test ends.
pub struct TempProfile { pub dir: PathBuf }

impl TempProfile {
    pub fn new(test: &str) -> TempProfile {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("ravenglass-test-{test}-{}-{nanos:x}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        TempProfile { dir }
    }

    pub fn profile(&self) -> Profile { Profile::at(&self.dir) }

    pub fn write(&self, file: &str, text: &str) {
        let path = self.dir.join(file);
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).expect("the folder"); }
        std::fs::write(path, text).expect("the file");
    }

    pub fn read(&self, file: &str) -> String { std::fs::read_to_string(self.dir.join(file)).unwrap_or_default() }
}

impl Drop for TempProfile {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
}

/// An IMAP account on this machine's port 1, where nothing listens: connecting fails at once
/// and no test ever reaches the network.
pub fn local_account(id: &str) -> AccountConfig {
    AccountConfig {
        id: AccountId::from(id),
        name: "Raven".into(),
        address: format!("{id}@localhost"),
        username: format!("{id}@localhost"),
        incoming: Incoming::Imap(Server::new("127.0.0.1", 1, Security::Plain)),
        smtp: Some(Server::new("127.0.0.1", 1, Security::Plain)),
        auth: AuthKind::Password,
        server_saves_sent: false,
    }
}

/// A profile-backed state with in-memory secrets and no network, loaded and run in place.
pub fn real_state(temp: &TempProfile, secrets: Arc<MemorySecrets>) -> AppState {
    let profile = temp.profile();
    let loaded = profile.load();
    AppState::real(profile, loaded, Services::offline(secrets), true)
}

/// A path from `/`-separated text, joined part by part so it compares equal on Windows too.
fn path(text: &str) -> PathBuf {
    let mut out = PathBuf::from(if text.starts_with('/') { "/" } else { "" });
    for part in text.split('/').filter(|p| !p.is_empty()) { out.push(part); }
    out
}

/// The parts of a path, whichever separator the host uses.
fn parts(p: &Path) -> Vec<String> {
    p.to_string_lossy().split(['/', '\\']).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: BTreeMap<String, String> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    move |key| map.get(key).cloned()
}

#[test]
fn linux_uses_the_xdg_folders_and_falls_back_to_the_home_folder() {
    let dirs = platform::linux::dirs(&env(&[("HOME", "/home/huginn")])).expect("a home");
    assert_eq!((parts(&dirs.config), parts(&dirs.data)), (parts(&path("/home/huginn/.config/ravenglass")), parts(&path("/home/huginn/.local/share/ravenglass"))));
    let dirs = platform::linux::dirs(&env(&[("HOME", "/home/huginn"), ("XDG_CONFIG_HOME", "/etc/xdg-huginn"), ("XDG_DATA_HOME", "/var/huginn")])).expect("a home");
    assert_eq!((parts(&dirs.config), parts(&dirs.data)), (parts(&path("/etc/xdg-huginn/ravenglass")), parts(&path("/var/huginn/ravenglass"))));
    // A relative XDG variable is ignored, as the spec says.
    let dirs = platform::linux::dirs(&env(&[("HOME", "/home/huginn"), ("XDG_CONFIG_HOME", "config")])).expect("a home");
    assert_eq!(parts(&dirs.config), parts(&path("/home/huginn/.config/ravenglass")));
    assert_eq!(platform::linux::dirs(&env(&[])), None, "no home, no profile");
}

#[test]
fn macos_and_windows_keep_settings_and_data_in_one_application_folder() {
    let mac = platform::macos::dirs(&env(&[("HOME", "/Users/huginn")])).expect("a home");
    assert_eq!(parts(&mac.config), parts(&path("/Users/huginn/Library/Application Support/Ravenglass")));
    assert_eq!(mac.config, mac.data);
    assert_eq!(platform::macos::dirs(&env(&[])), None);

    let win = platform::windows::dirs(&env(&[("APPDATA", r"C:\Users\huginn\AppData\Roaming")])).expect("APPDATA");
    assert_eq!(parts(&win.config), ["C:", "Users", "huginn", "AppData", "Roaming", "Ravenglass"]);
    assert_eq!(win.config, win.data);
    let fallback = platform::windows::dirs(&env(&[("USERPROFILE", r"C:\Users\huginn")])).expect("a home");
    assert_eq!(parts(&fallback.config), ["C:", "Users", "huginn", "AppData", "Roaming", "Ravenglass"]);
    assert_eq!(platform::windows::dirs(&env(&[])), None);
}

#[test]
fn the_profile_flag_wins_over_the_environment_which_wins_over_the_platform() {
    let e = env(&[("HOME", "/home/huginn"), ("RAVENGLASS_PROFILE", "/tmp/from-env")]);
    assert_eq!(Profile::resolve(Some("/tmp/from-flag"), &e), Some(Profile::at("/tmp/from-flag")));
    assert_eq!(Profile::resolve(None, &e), Some(Profile::at("/tmp/from-env")));
    // A home folder under every platform's own variable, so this runs on all three.
    let plain = env(&[("HOME", "/home/huginn"), ("APPDATA", r"C:\Users\huginn\AppData\Roaming"), ("RAVENGLASS_PROFILE", " ")]);
    let dirs = platform::dirs(&plain).expect("a home");
    assert_eq!(Profile::resolve(None, &plain), Some(Profile { config_dir: dirs.config, data_dir: dirs.data }));
    assert_eq!(Profile::resolve(None, &env(&[])), None);

    let one = Profile::at("/tmp/p");
    assert_eq!(one.config_file(), Profile::at("/tmp/p").config_dir.join("config.toml"));
    assert_eq!(one.accounts_file(), Profile::at("/tmp/p").config_dir.join("accounts.toml"));
    assert_eq!(one.themes_dir(), Profile::at("/tmp/p").config_dir.join("themes"));
    assert_eq!(one.store_file(), Profile::at("/tmp/p").config_dir.join("mail.db"));
}

#[test]
fn the_command_line_names_the_demo_and_the_profile() {
    let args = |list: &[&str]| parse_args(&list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    assert_eq!(args(&[]), profile::Options::default());
    assert!(args(&["--demo"]).demo);
    assert_eq!(args(&["--profile", "/tmp/p"]).profile.as_deref(), Some("/tmp/p"));
    assert_eq!(args(&["--profile=/tmp/q", "--demo"]), profile::Options { demo: true, profile: Some("/tmp/q".into()) });
}

fn sample_accounts() -> Vec<AccountConfig> {
    let mut gmail = local_account("gmail");
    gmail.incoming = Incoming::Imap(Server::new("imap.gmail.com", 993, Security::Tls));
    gmail.smtp = Some(Server::new("smtp.gmail.com", 465, Security::Tls));
    gmail.auth = AuthKind::OAuth2(Provider::Google);
    gmail.server_saves_sent = true;
    let mut fastmail = local_account("fastmail");
    fastmail.incoming = Incoming::Jmap { session_url: "https://api.fastmail.com/jmap/session".into() };
    fastmail.smtp = None;
    let mut icloud = local_account("icloud");
    icloud.smtp = Some(Server::new("smtp.mail.me.com", 587, Security::StartTls));
    vec![icloud, gmail, fastmail]
}

#[test]
fn accounts_toml_round_trips_every_kind_of_account() {
    let accounts = sample_accounts();
    let text = profile::accounts_to_toml(&accounts).expect("it serializes");
    assert_eq!(profile::accounts_from_toml(&text).expect("it parses"), accounts);
    assert!(text.contains("[[account]]"), "{text}");
    assert!(!text.to_lowercase().contains("secret") && !text.to_lowercase().contains("token"), "no secret has a place in the file: {text}");
    assert_eq!(profile::accounts_from_toml("").expect("an empty file"), vec![]);
}

#[test]
fn accounts_toml_refuses_bad_ids_duplicates_and_unknown_keys_without_quoting_the_file() {
    let twice = profile::accounts_to_toml(&[local_account("a"), local_account("a")]).expect("it serializes");
    assert!(profile::accounts_from_toml(&twice).unwrap_err().contains("twice"));
    let mut odd = local_account("a");
    odd.id = AccountId::from("../a");
    assert!(profile::accounts_from_toml(&profile::accounts_to_toml(&[odd]).expect("it serializes")).is_err());
    let unknown = profile::accounts_to_toml(&[local_account("a")]).expect("it serializes").replace("username", "password = \"hunter2\"\nusername");
    let error = profile::accounts_from_toml(&unknown).unwrap_err();
    assert!(!error.contains("hunter2") && !error.contains("@localhost"), "the error must not quote the file: {error}");
}

#[test]
fn saving_and_loading_stay_inside_the_profile_folder() {
    let temp = TempProfile::new("save-load");
    let profile = temp.profile();
    let accounts = sample_accounts();
    profile.save_accounts(&accounts).expect("saved");
    temp.write("config.toml", "[ui]\ntheme = \"ravenglass-frost\"\nundo_send_seconds = 3\n");
    temp.write("themes/dusk.toml", "name = \"Dusk\"\nbase = \"ravenglass-night\"\n[palette]\naqua = \"#FFAA00\"\n");
    temp.write("themes/broken.toml", "[palette]\naqua = \"not a colour\"\n");
    let loaded = profile.load();
    assert_eq!(loaded.accounts, accounts);
    assert_eq!(loaded.config.ui.undo_send_seconds, 3);
    assert_eq!(loaded.themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["dusk"]);
    assert_eq!(loaded.themes[0].color("aqua").hex(), "#FFAA00");
    assert_eq!(loaded.problems.len(), 1, "{:?}", loaded.problems);
    assert!(loaded.problems[0].starts_with("themes/broken.toml"), "{:?}", loaded.problems);

    // A theme name cannot climb out of the themes folder.
    let path = profile.save_theme("../../escape", "name = \"x\"\n").expect("saved");
    assert_eq!(path, temp.dir.join("themes").join("escape.toml"));
    assert!(profile.save_theme("../..", "").is_err());
    let mut files: Vec<String> = std::fs::read_dir(&temp.dir).expect("the folder").flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    assert_eq!(files, ["accounts.toml", "config.toml", "themes"]);
}

#[test]
fn a_broken_config_is_reported_and_the_defaults_are_used() {
    let temp = TempProfile::new("broken-config");
    temp.write("config.toml", "[ui]\ntheem = \"x\"\n");
    temp.write("accounts.toml", "[[account]]\nid = 1\n");
    let loaded = temp.profile().load();
    assert_eq!(loaded.config, Config::default());
    assert!(loaded.accounts.is_empty());
    assert_eq!(loaded.problems.len(), 2, "{:?}", loaded.problems);
    assert!(loaded.problems.iter().all(|p| !p.contains('\n')), "one line each: {:?}", loaded.problems);
    assert_eq!(temp.read("config.toml"), "[ui]\ntheem = \"x\"\n", "a broken file is left as it is");
}

#[test]
fn a_missing_profile_is_an_empty_one() {
    let temp = TempProfile::new("missing");
    let loaded: Loaded = Profile::at(temp.dir.join("not-there")).load();
    assert!(loaded.accounts.is_empty() && loaded.themes.is_empty() && loaded.problems.is_empty());
    assert!(!temp.dir.join("not-there").exists(), "reading makes nothing");
}

#[test]
fn a_user_theme_takes_its_id_from_its_file_and_cannot_replace_a_built_in() {
    let theme = profile::parse_user_theme("base = \"ravenglass-frost\"\n[palette]\nblue = \"#112233\"\n", "My Theme", &[]).expect("it parses");
    assert_eq!((theme.id.as_str(), theme.name.as_str()), ("my-theme", "My Theme"));
    assert!(profile::parse_user_theme("id = \"ravenglass-night\"\n", "x", &[]).is_err());
    assert!(profile::parse_user_theme("[colours]\n", "x", &[]).is_err(), "unknown tables are refused");
    // A whole theme written out reads back the same.
    let night = Theme::night();
    let mut copy = night.clone();
    copy.id = "copy".into();
    assert_eq!(profile::parse_user_theme(&profile::theme_to_toml(&copy), "copy", &[]).expect("it parses"), copy);
    // A terminal scheme with a light background is a light theme, whatever its base.
    let light = profile::parse_user_theme("base = \"ravenglass-night\"\n[palette]\nbackground = \"#FFFFFF\"\n", "paper", &[]).expect("it parses");
    assert_eq!(light.appearance, ravenglass_theme::Appearance::Light);
}

#[test]
fn config_toml_sets_the_theme_the_reader_and_the_privacy_choices() {
    let temp = TempProfile::new("apply");
    temp.write("config.toml", "[ui]\ntheme = \"ravenglass-frost\"\nreader_default = \"text\"\nhtml_dark = \"darken\"\n[privacy]\nremote_images = \"never\"\nlock_after_minutes = 5\n");
    let mut s = real_state(&temp, Arc::new(MemorySecrets::default()));
    assert_eq!(s.theme.id, "ravenglass-frost");
    assert_eq!(s.reader_view, ReaderView::Text);
    assert!(!s.images_offered());
    assert_eq!(s.html_dark(), DarkMode::Paper, "a light theme is never darkened");
    s.switch_theme("night");
    assert_eq!(s.html_dark(), DarkMode::Darken);
    assert_eq!(s.shell.lock.policy.idle_secs, 300.0);
    assert!(s.shell.onboarding(), "no accounts yet");
    assert!(s.toast.as_ref().is_none_or(|t| !t.error), "{:?}", s.toast);
}

#[test]
fn the_default_settings_keep_the_product_defaults() {
    let temp = TempProfile::new("defaults");
    let s = real_state(&temp, Arc::new(MemorySecrets::default()));
    assert_eq!(s.theme.id, "ravenglass-night");
    assert_eq!(s.reader_view, ReaderView::Html);
    assert_eq!(s.html_dark(), DarkMode::Paper);
    assert!(s.images_offered());
    assert!(!s.shell.lock.concealed());
}

#[test]
fn workspaces_come_from_the_config_or_one_per_account() {
    let temp = TempProfile::new("workspaces");
    temp.profile().save_accounts(&[local_account("home"), local_account("work")]).expect("saved");
    let s = real_state(&temp, Arc::new(MemorySecrets::default()));
    assert_eq!(s.workspaces.iter().map(|w| (w.slot, w.name.as_str())).collect::<Vec<_>>(), [(1, "home"), (2, "work")]);

    temp.write("config.toml", "[[workspace]]\nslot = 4\nsource = \"search:is:unread\"\n[[workspace]]\nslot = 2\nsource = \"account:work/folder:Receipts\"\n[[workspace]]\nslot = 7\nsource = \"account:gone\"\n");
    let s = real_state(&temp, Arc::new(MemorySecrets::default()));
    assert_eq!(s.workspaces.iter().map(|w| w.slot).collect::<Vec<_>>(), [2, 4]);
    assert_eq!(s.workspaces[0].source, Source::Mailbox { account: AccountId::from("work"), name: "Receipts".into() });
    assert_eq!(s.workspaces[1].source, Source::Search("is:unread".into()));
}

#[test]
fn system_theme_follows_the_appearance_the_window_reports() {
    let temp = TempProfile::new("system");
    temp.write("config.toml", "[ui]\ntheme = \"system\"\n");
    let mut s = real_state(&temp, Arc::new(MemorySecrets::default()));
    assert_eq!(s.theme.id, "ravenglass-night", "dark until the window says otherwise");
    s.shell.system_dark.set(Some(false));
    assert!(s.shell_poll(0.1));
    assert_eq!(s.theme.id, "ravenglass-frost");
    assert!(!s.shell_poll(0.2), "nothing to change twice");
    s.shell.system_dark.set(Some(true));
    assert!(s.shell_poll(0.3));
    assert_eq!(s.theme.id, "ravenglass-night");
    // Picking a theme by name stops the following.
    s.switch_theme("frost");
    assert!(!s.shell_poll(0.4));
    assert_eq!(s.theme.id, "ravenglass-frost");
}

#[test]
fn themes_are_imported_saved_and_fixed_for_contrast() {
    let temp = TempProfile::new("themes");
    let scheme = temp.dir.join("dim.conf");
    std::fs::write(&scheme, "background = #101010\nforeground = #303030\npalette = 7=#2a2a2a\npalette = 12=#252540\n").expect("the scheme");
    let mut s = real_state(&temp, Arc::new(MemorySecrets::default()));
    s.perform(crate::actions::parse_command(&format!("theme import {}", scheme.display())).expect("a command"));
    assert_eq!(s.theme.id, "dim", "{:?}", s.toast);
    assert!(temp.dir.join("themes/dim.toml").exists());
    assert!(!s.theme.contrast_report(4.5).is_empty(), "the scheme is too dim to read");

    s.perform(crate::actions::parse_command("theme fix-contrast").expect("a command"));
    assert_eq!(s.theme.id, "dim-fixed");
    assert_eq!(s.theme.contrast_report(4.5), vec![], "every text role passes after the fix");

    s.perform(crate::actions::parse_command("theme save Readable Dim").expect("a command"));
    assert_eq!(s.theme.id, "readable-dim");
    let reloaded = temp.profile().load();
    assert!(reloaded.problems.is_empty(), "{:?}", reloaded.problems);
    let saved = reloaded.themes.iter().find(|t| t.id == "readable-dim").expect("the saved theme");
    assert_eq!(saved.contrast_report(4.5), vec![]);
    // And it is there by name on the next start.
    let mut again = real_state(&temp, Arc::new(MemorySecrets::default()));
    again.switch_theme("Readable Dim");
    assert_eq!(again.theme.id, "readable-dim");

    s.perform(crate::actions::parse_command("theme import /no/such/file").expect("a command"));
    assert!(s.toast.as_ref().is_some_and(|t| t.error));
    s.perform(crate::actions::parse_command("theme save night").expect("a command"));
    assert!(s.toast.as_ref().is_some_and(|t| t.error), "a built-in name is refused");
}

#[test]
fn keys_are_rebound_from_the_config_and_clashes_are_refused() {
    use crate::actions::Action;
    use crate::keys::parse_seq;
    let keys = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<BTreeMap<String, String>>();
    let find = |table: &[crate::keys::Binding], action: Action| table.iter().find(|b| b.action == action).cloned().expect("the action");

    let table = keymap::build(&keys(&[("archive", "a"), ("palette", "mod+k"), ("focus-left", "ctrl+w a"), ("lock", "g l")])).expect("no clash");
    assert_eq!(find(&table, Action::Archive).normal, vec![parse_seq("a").expect("a key")]);
    assert_eq!(find(&table, Action::Archive).chords, find(&crate::keys::default_table(), Action::Archive).chords, "chords are kept");
    assert_eq!(find(&table, Action::Palette).chords, vec![parse_seq("mod+k").expect("a chord")]);
    assert_eq!(find(&table, Action::PaneFocus(crate::panes::Dir::Left)).normal, vec![parse_seq("ctrl+w a").expect("keys")]);
    assert_eq!(find(&table, Action::Shell(crate::shell::ShellCmd::Lock)).normal, vec![parse_seq("g l").expect("keys")], "an action with no built-in key can get one");

    assert!(keymap::build(&keys(&[("archive", "j")])).unwrap_err().contains("clashes"), "j is already next");
    assert!(keymap::build(&keys(&[("archive", "g")])).unwrap_err().contains("clashes"), "g starts g g");
    assert!(keymap::build(&keys(&[("archive", "j"), ("next", "n")])).is_ok(), "moving the other key away makes room");
    assert!(keymap::build(&keys(&[("teleport", "x")])).unwrap_err().contains("teleport"));
    assert!(keymap::build(&keys(&[("archive", "hyper+x")])).unwrap_err().contains("keys.archive"));
    assert_eq!(keymap::build(&BTreeMap::new()).expect("the built-in table").len(), crate::keys::default_table().len());
}

#[test]
fn a_key_clash_in_the_config_is_reported_and_the_built_in_keys_stay() {
    let temp = TempProfile::new("key-clash");
    temp.write("config.toml", "[keys]\narchive = \"j\"\n");
    let s = real_state(&temp, Arc::new(MemorySecrets::default()));
    let toast = s.toast.as_ref().expect("a toast");
    assert!(toast.error && toast.text.contains("clashes"), "{toast:?}");
    assert!(crate::keys::clashes().is_empty());
}

#[test]
fn remote_images_follow_the_privacy_setting() {
    use crate::actions::Action;
    use mail_core::machine::image_load::State;
    use ravenglass_config::RemoteImages;
    let open_newsletter = |policy: RemoteImages| {
        let mut s = AppState::demo_inline();
        s.shell.config.privacy.remote_images = policy;
        let row = s.rows().iter().position(|e| e.from.email.starts_with("news@dioxus")).expect("the newsletter");
        s.ws_mut().sel = row;
        s.open_selected(false);
        s
    };
    let open = |s: &AppState| s.ws().open.as_ref().expect("an open message").id.clone();
    let mut asked = open_newsletter(RemoteImages::Ask);
    assert!(matches!(asked.body().expect("a body").images, State::Blocked { .. }));
    assert!(asked.images_held(&open(&asked)), "held back until asked");
    asked.apply(Action::LoadImages { always: false });
    assert!(!asked.images_held(&open(&asked)), "the notice goes once they are loaded");
    let always = open_newsletter(RemoteImages::Always);
    assert!(always.shell.images_loaded.contains(&open(&always)) && !always.images_held(&open(&always)), "allowed without asking");
    let mut never = open_newsletter(RemoteImages::Never);
    assert!(!never.images_held(&open(&never)), "no offer to load them");
    never.apply(Action::LoadImages { always: false });
    assert!(never.shell.images_loaded.is_empty(), "the load key does nothing");
    assert!(never.toast.as_ref().is_some_and(|t| t.text.contains("remote images are off")));
}

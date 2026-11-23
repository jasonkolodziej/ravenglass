//! `config.toml` applied to the running app (architecture appendix A, §11): the theme (with
//! `system` following the OS appearance), the reader's default view, the HTML dark mode, the
//! remote-image policy, the workspace bar, the key table and the lock policy. Themes are
//! switched, imported, saved and contrast-fixed here too.

use crate::jobs::Job;
use crate::profile;
use crate::state::{AppState, Fx, Source, Workspace};
use mail_auth::AccountConfig;
use mail_core::machine::app_lock::Policy;
use mail_core::machine::image_load;
use mail_core::{AccountId, MessageId};
use ravenglass_config::{Config, HtmlDark, ReaderDefault, RemoteImages};
use ravenglass_engine::{Command, DarkMode, ReaderView};
use ravenglass_theme::{Appearance, Rgb, Theme, contrast, fix_contrast};

/// The contrast every text role should have against the background (WCAG AA).
pub const MIN_CONTRAST: f32 = 4.5;

pub fn lock_policy(config: &Config) -> Policy {
    Policy { idle_secs: f64::from(config.privacy.lock_after_minutes) * 60.0, lock_on_launch: config.privacy.lock_on_launch }
}

/// The workspace bar: the config's `[[workspace]]` slots, or one per account's Inbox. Slots
/// that name an account that is not set up are left out and reported. Never empty: with no
/// account there is one placeholder, which the onboarding pane covers.
pub fn workspaces_for(config: &Config, accounts: &[AccountConfig]) -> (Vec<Workspace>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let known = |id: &str| accounts.iter().find(|a| a.id.0.eq_ignore_ascii_case(id)).map(|a| a.id.clone());
    for slot in &config.workspaces {
        if let Some(query) = slot.source.strip_prefix("search:") {
            out.push(Workspace::new(slot.slot, query.trim(), Source::Search(query.trim().to_owned())));
            continue;
        }
        let rest = slot.source.strip_prefix("account:").unwrap_or("");
        let (id, folder) = match rest.split_once("/folder:") { Some((id, folder)) => (id, Some(folder)), None => (rest, None) };
        match known(id.trim()) {
            Some(account) => {
                let name = folder.map(str::trim).filter(|f| !f.is_empty());
                out.push(Workspace::new(slot.slot, name.unwrap_or(&account.0), Source::Mailbox { account: account.clone(), name: name.unwrap_or("Inbox").to_owned() }));
            }
            None => notes.push(format!("workspace {}: no account named {}", slot.slot, id.trim())),
        }
    }
    if out.is_empty() {
        for (i, account) in accounts.iter().take(9).enumerate() {
            out.push(Workspace::new(i as u8 + 1, &account.id.0, Source::Mailbox { account: account.id.clone(), name: "Inbox".into() }));
        }
    }
    if out.is_empty() { out.push(Workspace::new(1, "welcome", Source::Mailbox { account: AccountId::from("ravenglass"), name: "welcome".into() })); }
    out.sort_by_key(|w| w.slot);
    (out, notes)
}

impl AppState {
    /// Take what was read from the profile: settings, accounts and user themes. Problems (a
    /// file that does not parse, a key that clashes) are shown, never fatal.
    pub fn apply_loaded(&mut self, loaded: profile::Loaded) {
        let profile::Loaded { config, accounts, themes, mut problems } = loaded;
        self.shell.config = config;
        self.shell.accounts = accounts;
        self.shell.themes = themes;
        problems.extend(crate::keymap::apply(&self.shell.config.keys));
        self.reader_view = match self.shell.config.ui.reader_default { ReaderDefault::Html => ReaderView::Html, ReaderDefault::Text => ReaderView::Text };
        let wanted = self.shell.config.theme_for(self.shell.system_dark.get().unwrap_or(true)).to_owned();
        match self.find_theme(&wanted) {
            Some(theme) => self.theme = theme,
            None => problems.push(format!("config.toml: no theme named {wanted}")),
        }
        self.shell.lock.set_policy(lock_policy(&self.shell.config), self.clock);
        let (workspaces, _) = workspaces_for(&self.shell.config, &self.shell.accounts);
        self.workspaces = workspaces;
        self.active = self.active.min(self.workspaces.len() - 1);
        if let Some(first) = problems.first() {
            let more = if problems.len() > 1 { format!(" (+{} more)", problems.len() - 1) } else { String::new() };
            self.error(format!("{first}{more}"));
        }
    }

    /// A theme by id or name: the built-ins (`night`, `ravenglass-frost`), then the user's.
    pub fn find_theme(&self, name: &str) -> Option<Theme> {
        let wanted = name.trim().to_lowercase();
        match wanted.as_str() {
            "night" | "ravenglass-night" => return Some(Theme::night()),
            "frost" | "ravenglass-frost" => return Some(Theme::frost()),
            _ => {}
        }
        self.shell.themes.iter().find(|t| t.id == wanted || t.name.to_lowercase() == wanted).cloned()
    }

    /// `:theme <name>`. `system` follows the OS appearance with the config's light and dark
    /// choices. The choice lasts for this run; `config.toml` is the user's to edit.
    pub fn switch_theme(&mut self, name: &str) {
        let name = name.trim();
        if name.eq_ignore_ascii_case("system") {
            self.shell.config.ui.theme = "system".into();
            let wanted = self.shell.config.theme_for(self.shell.system_dark.get().unwrap_or(true)).to_owned();
            return match self.find_theme(&wanted) {
                Some(theme) => {
                    self.toast(format!("theme: system ({})", theme.name));
                    self.show_theme(theme);
                }
                None => self.error(format!("no theme named {wanted}")),
            };
        }
        match self.find_theme(name) {
            Some(theme) => {
                self.shell.config.ui.theme = theme.id.clone();
                self.toast(format!("theme: {}", theme.name));
                self.show_theme(theme);
            }
            None => self.error(format!("no theme named {name}")),
        }
    }

    fn show_theme(&mut self, theme: Theme) {
        let was_dark = self.theme.appearance == Appearance::Dark;
        let now_dark = theme.appearance == Appearance::Dark;
        self.theme = theme;
        self.fx.push(Fx::ThemeChanged);
        // The darkened HTML view is painted for a dark theme only: paint the open message again.
        if was_dark != now_dark && self.shell.config.ui.html_dark == HtmlDark::Darken && let Some(id) = self.ws().open.as_ref().map(|e| e.id.clone()) {
            self.dispatch(Command::OpenMessage { id, view: self.reader_view });
        }
    }

    /// A theme that was just imported, saved or fixed: keep it among the user's and show it.
    pub fn adopt_theme(&mut self, theme: Theme) {
        self.shell.themes.retain(|t| t.id != theme.id);
        self.shell.themes.push(theme.clone());
        self.shell.config.ui.theme = theme.id.clone();
        self.toast(format!("theme: {}", theme.name));
        self.show_theme(theme);
    }

    /// With `ui.theme = "system"`: switch when the OS appearance the view saw changes.
    pub fn follow_system(&mut self) -> bool {
        if self.shell.config.ui.theme != "system" { return false; }
        let Some(dark) = self.shell.system_dark.get() else { return false };
        let wanted = self.shell.config.theme_for(dark).to_owned();
        if self.theme.id == wanted { return false; }
        match self.find_theme(&wanted) {
            Some(theme) => {
                self.show_theme(theme);
                true
            }
            None => false,
        }
    }

    /// `:theme fix-contrast`: nudge the lightness of every text colour that fails 4.5:1 against
    /// the background until it passes. The result is a new theme in memory; `:theme save` keeps it.
    pub fn fix_contrast(&mut self) {
        let failing = self.theme.contrast_report(MIN_CONTRAST);
        if failing.is_empty() { return self.toast("every text colour already passes 4.5:1"); }
        let mut theme = self.theme.clone();
        let bg = theme.color("background");
        for (role, _) in &failing {
            let Some(name) = theme.roles.get(role).cloned() else { continue };
            // The background itself cannot be made to contrast with itself: use the foreground.
            if name == "background" {
                theme.roles.insert(role.clone(), "foreground".into());
                continue;
            }
            // Aim a little past the line: the colour is saved as 8-bit hex, and rounding must
            // not bring it back under.
            let fixed = fix_contrast(theme.color(&name), bg, MIN_CONTRAST + 0.1);
            theme.palette.insert(name, Rgb::parse(&fixed.hex()).unwrap_or(fixed));
        }
        let still = theme.contrast_report(MIN_CONTRAST).len();
        if !theme.id.ends_with("-fixed") {
            theme.id = format!("{}-fixed", theme.id);
            theme.name = format!("{} (fixed)", theme.name);
        }
        let name = theme.name.clone();
        self.adopt_theme(theme);
        let fixed = failing.len() - still;
        self.toast(format!("fixed {fixed} of {} roles · :theme save <name> keeps {name}", failing.len()));
    }

    /// `:theme save <name>`: the theme on screen, whole, as `themes/<name>.toml`.
    pub fn save_theme_as(&mut self, name: &str) {
        let id = profile::slug(name);
        if id.is_empty() { return self.error("the theme needs a name"); }
        if Theme::builtin(&id).is_some() || matches!(id.as_str(), "night" | "frost" | "system") { return self.error(format!("{id} is a built-in theme: pick another name")); }
        let mut theme = self.theme.clone();
        theme.id = id;
        theme.name = name.trim().to_owned();
        self.spawn_job(Job::SaveTheme { theme: Box::new(theme) });
    }

    /// `:theme import <file>`: a Ghostty, Alacritty, Kitty, Windows Terminal or iTerm2 colour
    /// scheme (or a Ravenglass theme file), layered on the built-in theme of the same appearance.
    pub fn import_theme(&mut self, path: String) {
        let base = match self.theme.appearance { Appearance::Dark => "ravenglass-night", Appearance::Light => "ravenglass-frost" };
        self.spawn_job(Job::ImportTheme { path, base: base.into(), known: self.shell.themes.clone() });
    }

    /// How HTML mail is painted: darkened only when the setting asks and the theme is dark.
    pub fn html_dark(&self) -> DarkMode {
        if self.shell.config.ui.html_dark == HtmlDark::Darken && self.theme.appearance == Appearance::Dark { DarkMode::Darken } else { DarkMode::Paper }
    }

    /// Whether the reader offers to load remote images (`privacy.remote_images = "never"` hides it).
    pub fn images_offered(&self) -> bool { self.shell.config.privacy.remote_images != RemoteImages::Never }

    /// The reader says "remote images blocked" for this message: they are held back and the
    /// user may load them.
    pub fn images_held(&self, message: &MessageId) -> bool { self.images_offered() && !self.shell.images_loaded.contains(message) }

    /// The engine let a message's images through.
    pub fn images_allowed(&mut self, message: MessageId) {
        if self.shell.images_loaded.len() >= 512 { self.shell.images_loaded.clear(); }
        self.shell.images_loaded.insert(message);
    }

    /// `privacy.remote_images = "always"`: a message that opens with images held back gets them.
    pub fn auto_images(&mut self, message: &MessageId) {
        if self.shell.config.privacy.remote_images != RemoteImages::Always { return; }
        let blocked = self.mail.bodies.get(message).is_some_and(|body| matches!(body.images, image_load::State::Blocked { .. }));
        if blocked { self.dispatch(Command::AllowImages { message: message.clone(), scope: image_load::Scope::Once }); }
    }

    /// Each text role with its contrast against the background, for the `:colors` pane.
    pub fn role_contrasts(&self) -> Vec<(String, String, f32)> {
        let bg = self.theme.color("background");
        self.theme.roles.iter().map(|(role, name)| (role.clone(), name.clone(), contrast(self.theme.color(name), bg))).collect()
    }
}

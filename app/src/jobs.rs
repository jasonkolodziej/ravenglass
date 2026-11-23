//! Work the shell does off the UI thread: autoconfig lookups, the browser sign-in, the secret
//! store, and every write to the profile. A [`Job`] runs on a thread of its own in the window and
//! in place in headless runs and tests; either way its answer comes back as a [`JobDone`] that
//! `shell.rs` folds into the state.
//!
//! Nothing here formats a secret into an error: failures are described by kind.

use crate::accounts_ui::Secret;
use crate::link::Link;
use crate::platform;
use crate::profile::{self, Loaded, Profile};
use crate::shell::Services;
use mail_auth::autoconfig::{self, Settings};
use crate::platform::accounts::Redirect;
use mail_auth::oauth;
use mail_auth::{AccountConfig, AuthKind, SecretKind};
use mail_core::AccountId;
use ravenglass_engine::runner;
use ravenglass_theme::{Theme, ThemeFile};
use std::sync::mpsc::Sender;

pub enum Job {
    Discover { email: String },
    /// Store the password (if any) and write `accounts.toml` as `accounts`. `new`: the account
    /// was not there before, so a failed save takes its secrets back out.
    Save { config: AccountConfig, password: Option<Secret>, accounts: Vec<AccountConfig>, new: bool },
    SignIn { config: AccountConfig },
    /// Stop the engine, write `accounts.toml` without the account, forget its secrets, and
    /// delete the mail cache when no account is left.
    Remove { id: AccountId, remaining: Vec<AccountConfig>, link: Link },
    /// The portable app-lock check: `typed` against the account's stored password.
    Verify { account: AccountId, typed: Secret },
    ImportTheme { path: String, base: String, known: Vec<Theme> },
    SaveTheme { theme: Box<Theme> },
    Reload,
}

pub enum JobDone {
    Discovered(Result<Settings, String>),
    Saved(Result<AccountConfig, String>),
    /// Open the provider's sign-in page; `redirect` is the loopback address waiting for it.
    OpenUrl { url: String, redirect: String },
    SignedIn(Result<(), String>),
    Removed { id: AccountId, result: Result<(), String>, cache_cleared: bool },
    Verified(bool),
    ThemeImported(Result<Box<Theme>, String>),
    ThemeSaved(Result<Box<Theme>, String>),
    Reloaded(Box<Loaded>),
    /// The engine was built: accounts without a backend, and the guard of the token refresher.
    Connected { problems: Vec<(AccountId, String)>, tokens: Option<Sender<()>> },
}

/// What a job may use. Cheap to clone onto the job's thread.
#[derive(Clone)]
pub struct Ctx {
    pub services: Services,
    pub profile: Option<Profile>,
    /// Running in place on the UI thread (headless, tests): nothing may wait on a person.
    pub inline: bool,
}

pub fn run(job: Job, ctx: &Ctx, emit: &mut dyn FnMut(JobDone)) {
    match job {
        Job::Discover { email } => {
            let fetch = ctx.services.fetch.clone();
            let found = autoconfig::discover(&email, &|url| fetch(url)).map_err(|e| e.to_string());
            emit(JobDone::Discovered(found));
        }
        Job::Save { config, password, accounts, new } => emit(JobDone::Saved(save(ctx, config, password, &accounts, new))),
        Job::SignIn { config } => {
            let result = sign_in(ctx, &config, emit);
            emit(JobDone::SignedIn(result));
        }
        Job::Remove { id, remaining, link } => {
            link.shutdown();
            let saved = match &ctx.profile { Some(profile) => profile.save_accounts(&remaining), None => Ok(()) };
            let mut cache_cleared = false;
            let result = saved.and_then(|()| {
                cache_cleared = match &ctx.profile {
                    // The last account takes the whole cache file with it; otherwise only its own rows go.
                    Some(profile) if remaining.is_empty() => profile.remove_store().is_ok(),
                    Some(profile) => mail_store::Store::open(&profile.store_file().to_string_lossy()).and_then(|mut store| store.remove_account(&id)).is_ok(),
                    None => false,
                };
                ctx.services.secrets.delete_all(&id).map_err(|e| e.to_string())
            });
            emit(JobDone::Removed { id, result, cache_cleared });
        }
        Job::Verify { account, typed } => emit(JobDone::Verified(platform::check_password(ctx.services.secrets.as_ref(), &account, typed.expose()))),
        Job::ImportTheme { path, base, known } => emit(JobDone::ThemeImported(import_theme(ctx, &path, &base, &known).map(Box::new))),
        Job::SaveTheme { theme } => {
            let saved = match &ctx.profile {
                Some(profile) => profile.save_theme(&theme.id, &profile::theme_to_toml(&theme)).map(|_| theme),
                None => Err("the demo has no profile to save themes in".into()),
            };
            emit(JobDone::ThemeSaved(saved));
        }
        Job::Reload => emit(JobDone::Reloaded(Box::new(ctx.profile.as_ref().map(Profile::load).unwrap_or_default()))),
    }
}

fn save(ctx: &Ctx, config: AccountConfig, password: Option<Secret>, accounts: &[AccountConfig], new: bool) -> Result<AccountConfig, String> {
    let secrets = &ctx.services.secrets;
    if let Some(password) = &password {
        secrets.set(&config.id, SecretKind::Password, password.expose()).map_err(|e| e.to_string())?;
    }
    drop(password);
    if let Some(profile) = &ctx.profile && let Err(problem) = profile.save_accounts(accounts) {
        if new { let _ = secrets.delete_all(&config.id); }
        return Err(problem);
    }
    Ok(config)
}

/// The loopback OAuth flow (RFC 8252): listen on a port of this machine, send the browser to
/// the provider, wait for it to come back, and trade the code for a refresh token that goes
/// straight to the `SecretStore`.
fn sign_in(ctx: &Ctx, config: &AccountConfig, emit: &mut dyn FnMut(JobDone)) -> Result<(), String> {
    if ctx.inline { return Err("the browser sign-in needs the window".into()); }
    let AuthKind::OAuth2(provider) = config.auth else { return Err("this account does not use a browser sign-in".into()) };
    let ids = &ctx.services.client_ids;
    let client_id = match provider { oauth::Provider::Google => &ids.google, oauth::Provider::Microsoft => &ids.microsoft };
    let client_id = client_id.as_deref().ok_or_else(|| format!("this build has no {} OAuth client id", crate::accounts_ui::provider_name(provider)))?;
    let attempt = oauth::Attempt::new().map_err(|e| e.to_string())?;
    let redirect = Redirect::listen().map_err(|e| e.to_string())?;
    let uri = redirect.uri().map_err(|e| e.to_string())?;
    emit(JobDone::OpenUrl { url: attempt.authorize_url(provider, client_id, &uri, &config.address), redirect: uri.clone() });
    let query = redirect.wait().map_err(|e| e.to_string())?;
    ctx.services.connector().complete_sign_in(config, &attempt, &uri, &query, runner::now()).map_err(|e| e.to_string().lines().next().unwrap_or("the sign-in failed").to_owned())
}

/// Give up a sign-in that waits for the browser: answer the loopback listener ourselves with a
/// refusal, so its thread ends. `redirect` is the listener's own `http://127.0.0.1:<port>/`.
pub fn cancel_sign_in(redirect: &str) {
    use std::io::Write;
    let Some(port) = redirect.strip_prefix("http://127.0.0.1:").and_then(|rest| rest.trim_end_matches('/').parse::<u16>().ok()) else { return };
    if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
        let _ = stream.write_all(b"GET /?error=cancelled HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    }
}

fn import_theme(ctx: &Ctx, path: &str, base: &str, known: &[Theme]) -> Result<Theme, String> {
    let file = std::path::Path::new(path);
    let src = profile::read_capped(file)?.ok_or("there is no such file")?;
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    // A Ravenglass theme file is taken as it is; anything else is a terminal colour scheme.
    let native = toml::from_str::<ThemeFile>(&src).is_ok_and(|f| !f.palette.is_empty() || !f.roles.is_empty());
    let toml = if native { src } else { ravenglass_theme::import(&src, &stem, base).map_err(|e| e.to_string())? };
    if !toml::from_str::<ThemeFile>(&toml).is_ok_and(|f| !f.palette.is_empty() || !f.roles.is_empty()) { return Err("no colours were found in that file".into()); }
    let theme = profile::parse_user_theme(&toml, &stem, known)?;
    if let Some(profile) = &ctx.profile { profile.save_theme(&theme.id, &toml)?; }
    Ok(theme)
}

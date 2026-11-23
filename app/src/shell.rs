//! The shell around the mail UI: the profile, the accounts and their secrets, the settings, the
//! account form and the app lock. `AppState` holds one [`Shell`]; everything the rest of the app
//! needs from it goes through a handful of hooks (`shell_key`, `shell_poll`, `shell_cmd`,
//! `status_line`, `start_real_engine`), so the mail code stays as it was on the demo accounts.

use crate::accounts_ui::{self, Flow, FlowEffect, FlowEvent, Form, Providers};
use crate::jobs::{self, Ctx, Job, JobDone};
use crate::keys::{K, KeyPress};
use crate::link::{self, EngineSpec, Link};
use crate::lock::{Asked, Lock};
use crate::modes::Mode;
use crate::profile::{Loaded, Options, PROFILE_ENV, Profile};
use crate::state::{AppState, Boot, Fx, Mail};
use mail_auth::autoconfig;
use mail_auth::{AccountConfig, AuthKind, MemorySecrets, SecretStore};
use mail_core::AccountId;
use ravenglass_accounts::{ClientIds, Connector, FormPost};
use ravenglass_config::Config;
use ravenglass_engine::runner;
use ravenglass_theme::Theme;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// Fetches an autoconfig document; `None` on any failure.
pub type Fetch = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// What reaches outside the process: the secret store and HTTPS. Tests pass in-memory ones.
#[derive(Clone)]
pub struct Services {
    pub secrets: Arc<dyn SecretStore>,
    pub client_ids: ClientIds,
    pub fetch: Fetch,
    /// How OAuth token requests are sent. `None`: OAuth accounts cannot connect.
    pub post: Option<FormPost>,
}

impl Services {
    /// No network, and the given secret store: tests, and the demo.
    pub fn offline(secrets: Arc<dyn SecretStore>) -> Services {
        Services { secrets, client_ids: ClientIds::default(), fetch: Arc::new(|_| None), post: None }
    }

    /// The desktop services: the OS keyring (or memory with `RAVENGLASS_SECRETS=memory`, for
    /// tests and headless runs), HTTPS, and the OAuth client ids this run was started with.
    pub fn from_env(env: crate::platform::Env) -> Services {
        let secrets: Arc<dyn SecretStore> = if env("RAVENGLASS_SECRETS").as_deref() == Some("memory") { Arc::new(MemorySecrets::default()) } else { Arc::new(mail_auth::secrets::KeyringSecrets::new()) };
        let id = |key: &str| env(key).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let client_ids = ClientIds { google: id(accounts_ui::client_id_env(mail_auth::oauth::Provider::Google)), microsoft: id(accounts_ui::client_id_env(mail_auth::oauth::Provider::Microsoft)) };
        Services { secrets, client_ids, fetch: Arc::new(ravenglass_accounts::http::fetch_document), post: Some(ravenglass_accounts::http::form_post()) }
    }

    pub fn connector(&self) -> Connector {
        Connector { secrets: self.secrets.clone(), client_ids: self.client_ids.clone(), post: self.post.clone() }
    }

    pub fn providers(&self) -> Providers {
        Providers { google: self.client_ids.google.is_some(), microsoft: self.client_ids.microsoft.is_some() }
    }
}

/// `:` commands that belong to the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellCmd {
    AccountAdd(String),
    AccountRemove(String),
    /// Type a (new) password for an account that has none stored.
    AccountPassword(String),
    AccountSignIn(String),
    AccountList,
    Colors,
    Lock,
    ThemeImport(String),
    ThemeSave(String),
    ThemeFixContrast,
    Reload,
}

/// The shell's part of the command line. `None`: not one of its commands.
pub fn parse(head: &str, rest: &str) -> Option<Result<ShellCmd, String>> {
    let (sub, arg) = rest.split_once(char::is_whitespace).map_or((rest, ""), |(s, a)| (s, a.trim()));
    let need = |what: &str, make: fn(String) -> ShellCmd| if arg.is_empty() { Err(format!(":{head} {sub} needs {what}")) } else { Ok(make(arg.to_owned())) };
    Some(match (head, sub) {
        ("account" | "accounts", "" | "list") => Ok(ShellCmd::AccountList),
        ("account", "add") => need("an email address", ShellCmd::AccountAdd),
        ("account", "remove" | "rm") => need("an account id", ShellCmd::AccountRemove),
        ("account", "password") => need("an account id", ShellCmd::AccountPassword),
        ("account", "signin") => need("an account id", ShellCmd::AccountSignIn),
        ("account", _) => Err(":account takes add, remove, password, signin or list".into()),
        ("colors" | "colours", _) => Ok(ShellCmd::Colors),
        ("lock", _) => Ok(ShellCmd::Lock),
        ("reload", _) => Ok(ShellCmd::Reload),
        ("theme", "import") => need("a file", ShellCmd::ThemeImport),
        ("theme", "save") => need("a name", ShellCmd::ThemeSave),
        ("theme", "fix-contrast") => Ok(ShellCmd::ThemeFixContrast),
        _ => return None,
    })
}

pub struct Shell {
    /// Running on a profile (not the demo accounts).
    pub real: bool,
    /// Jobs run in place: headless runs and tests.
    pub inline: bool,
    pub profile: Option<Profile>,
    pub services: Services,
    pub config: Config,
    pub accounts: Vec<AccountConfig>,
    /// Accounts without a backend, each with the one line that says why.
    pub problems: BTreeMap<AccountId, String>,
    /// User themes from `themes/*.toml`, and ones imported or fixed in this run.
    pub themes: Vec<Theme>,
    /// The `:colors` pane is up.
    pub colors: bool,
    /// The add-account form, when it is up.
    pub flow: Option<Flow>,
    pub lock: Lock,
    /// The OS appearance, as the last view run saw it.
    pub system_dark: Cell<Option<bool>>,
    /// The loopback address of a sign-in waiting for the browser.
    pub signin: Option<String>,
    /// Keeps the token-refresh thread alive; dropping it ends the thread.
    pub tokens: Option<Sender<()>>,
    lock_wake: Option<f64>,
    jobs_tx: Sender<JobDone>,
    jobs_rx: Receiver<JobDone>,
}

impl Default for Shell {
    fn default() -> Self {
        let (jobs_tx, jobs_rx) = channel();
        Shell {
            real: false,
            inline: false,
            profile: None,
            services: Services::offline(Arc::new(MemorySecrets::default())),
            config: Config::default(),
            accounts: Vec::new(),
            problems: BTreeMap::new(),
            themes: Vec::new(),
            colors: false,
            flow: None,
            lock: Lock::default(),
            system_dark: Cell::new(None),
            signin: None,
            tokens: None,
            lock_wake: None,
            jobs_tx,
            jobs_rx,
        }
    }
}

impl Shell {
    /// No account yet: the onboarding pane shows in place of the mail panes.
    pub fn onboarding(&self) -> bool { self.real && self.accounts.is_empty() }

    /// The account whose password opens the app lock: the first one that has a password.
    pub fn verifier(&self) -> Option<AccountId> {
        self.accounts.iter().find(|a| a.auth == AuthKind::Password).map(|a| a.id.clone())
    }

    /// What the workspace bar says in place of the sync state, when the shell knows better.
    pub fn status_line(&self) -> Option<String> {
        if self.onboarding() { return Some("no accounts".into()); }
        let (id, problem) = self.problems.iter().next()?;
        let more = if self.problems.len() > 1 { format!(" (+{})", self.problems.len() - 1) } else { String::new() };
        Some(format!("{id}: {problem}{more}"))
    }
}

impl AppState {
    /// The state the app starts in, from the command line and the environment: the demo
    /// (`--demo`, and headless runs that name no profile) or the profile on disk.
    pub fn launch() -> AppState {
        let env = |key: &str| std::env::var(key).ok();
        let options = crate::profile::parse_args(rune::tooling::extra_args());
        let headless = rune::tooling::is_headless();
        match chosen_profile(&options, &env, headless) {
            // Headless runs load everything inline so the first frame already shows mail.
            None if headless => AppState::demo_inline(),
            None => AppState::windowed(),
            // Reading the profile is the one piece of file I/O on this thread: it happens once,
            // before the first frame.
            Some(profile) => {
                let loaded = profile.load();
                AppState::real(profile, loaded, Services::from_env(&env), headless)
            }
        }
    }

    /// A profile-backed state. `inline`: build the engine and run jobs in place (headless runs
    /// and tests); otherwise the engine starts on its own thread after the first frame.
    pub fn real(profile: Profile, loaded: Loaded, services: Services, inline: bool) -> AppState {
        let mut s = AppState::windowed();
        s.shell.real = true;
        s.shell.inline = inline;
        s.shell.profile = Some(profile);
        s.shell.services = services;
        s.apply_loaded(loaded);
        s.shell.lock = Lock::launch(crate::settings::lock_policy(&s.shell.config), 0.0, s.shell.verifier().is_some());
        if inline { s.start_real_engine(); }
        s
    }

    fn job_ctx(&self) -> Ctx {
        Ctx { services: self.shell.services.clone(), profile: self.shell.profile.clone(), inline: self.runs_inline() }
    }

    fn runs_inline(&self) -> bool { self.shell.inline || matches!(self.link, Link::Inline { .. }) }

    /// Run a job off the UI thread (in place when headless), and fold its answers when they come.
    pub fn spawn_job(&mut self, job: Job) {
        let ctx = self.job_ctx();
        if ctx.inline {
            let mut done = Vec::new();
            jobs::run(job, &ctx, &mut |answer| done.push(answer));
            for answer in done { self.fold_job(answer); }
            return;
        }
        let (tx, waker) = (self.shell.jobs_tx.clone(), self.waker.clone());
        let work = move || {
            jobs::run(job, &ctx, &mut |answer| {
                let _ = tx.send(answer);
                if let Some(wake) = &waker { wake(); }
            });
        };
        if std::thread::Builder::new().name("ravenglass-job".into()).spawn(work).is_err() { self.error("no thread to do that on"); }
    }

    /// Build the engine over the profile's store and open every account that connects. An
    /// account that does not (no password stored, sign-in needed) is named in the workspace
    /// bar and does not stop the others.
    pub fn start_real_engine(&mut self) {
        self.boot = Boot::Started;
        self.mail = Mail::default();
        self.html = Default::default();
        self.undo.clear();
        self.undoing = None;
        self.pending.clear();
        self.shell.problems.clear();
        self.shell.tokens = None;
        let (workspaces, notes) = crate::settings::workspaces_for(&self.shell.config, &self.shell.accounts);
        self.workspaces = workspaces;
        self.active = self.active.min(self.workspaces.len() - 1);
        if let Some(note) = notes.into_iter().next() { self.error(note); }
        if self.shell.accounts.is_empty() {
            self.link = Link::Off;
            return;
        }
        let spec = EngineSpec {
            store: self.shell.profile.as_ref().map(Profile::store_file),
            accounts: self.shell.accounts.clone(),
            undo_send_secs: f64::from(self.shell.config.ui.undo_send_seconds),
        };
        let (services, profile) = (self.shell.services.clone(), self.shell.profile.clone());
        let prepare = move || match &profile { Some(profile) => profile.ensure_dirs(), None => Ok(()) };
        if self.runs_inline() || self.shell.inline {
            let now = runner::now();
            match prepare().and_then(|()| link::real_engine(&spec, &services, now)) {
                Ok(built) => {
                    self.link = Link::Inline { engine: Box::new(built.engine), now };
                    let first = link::first_commands(&built.connected);
                    self.fold_job(JobDone::Connected { problems: built.problems, tokens: None });
                    for cmd in first { self.dispatch(cmd); }
                }
                Err(reason) => {
                    self.link = Link::Off;
                    self.error(format!("engine: {reason}"));
                }
            }
            return;
        }
        let Some(waker) = self.waker.clone() else { return };
        let answers = self.shell.jobs_tx.clone();
        let build = move || {
            prepare()?;
            let built = link::real_engine(&spec, &services, runner::now())?;
            let first = link::first_commands(&built.connected);
            let tokens = link::keep_tokens_fresh(services.connector(), built.connected);
            let _ = answers.send(JobDone::Connected { problems: built.problems, tokens });
            Ok((built.engine, first))
        };
        self.link = Link::start_with(build, move || waker());
    }

    /// Stop the engine and start it again over the current accounts (one was added or changed).
    fn restart_engine(&mut self) {
        let old = std::mem::replace(&mut self.link, Link::Off);
        if self.runs_inline() || self.shell.inline {
            drop(old);
        } else {
            // Joining the engine thread can wait on a sync in flight; not on this thread.
            let _ = std::thread::Builder::new().name("ravenglass-stop".into()).spawn(move || old.shutdown());
        }
        self.start_real_engine();
    }

    fn fold_job(&mut self, done: JobDone) {
        match done {
            JobDone::Discovered(found) => self.flow_event(FlowEvent::Discovered(found)),
            JobDone::Saved(Ok(config)) => {
                match self.shell.accounts.iter_mut().find(|a| a.id == config.id) {
                    Some(existing) => *existing = config,
                    None => self.shell.accounts.push(config),
                }
                match self.shell.flow {
                    Some(_) => self.flow_event(FlowEvent::Saved(Ok(()))),
                    None => self.toast("account saved"),
                }
                self.restart_engine();
            }
            JobDone::Saved(Err(reason)) => match self.shell.flow {
                Some(_) => self.flow_event(FlowEvent::Saved(Err(reason))),
                None => self.error(reason),
            },
            JobDone::OpenUrl { url, redirect } => {
                self.shell.signin = Some(redirect);
                self.fx.push(Fx::OpenUrl(url));
            }
            JobDone::SignedIn(result) => {
                self.shell.signin = None;
                self.flow_event(FlowEvent::SignedIn(result));
            }
            JobDone::Removed { id, result, cache_cleared } => {
                match result {
                    Ok(()) => {
                        self.shell.accounts.retain(|a| a.id != id);
                        self.toast(if cache_cleared { format!("account {id} removed, with its secrets and the mail cache") } else { format!("account {id} and its secrets removed · its cached mail stays until the last account goes") });
                    }
                    Err(reason) => self.error(format!("account {id}: {reason}")),
                }
                self.start_real_engine();
            }
            JobDone::Verified(ok) => {
                let can_verify = self.shell.verifier().is_some();
                self.shell.lock.verified(ok, self.clock, can_verify);
            }
            JobDone::ThemeImported(Ok(theme)) | JobDone::ThemeSaved(Ok(theme)) => self.adopt_theme(*theme),
            JobDone::ThemeImported(Err(reason)) | JobDone::ThemeSaved(Err(reason)) => self.error(format!("theme: {reason}")),
            JobDone::Reloaded(loaded) => {
                let accounts = self.shell.accounts.clone();
                self.apply_loaded(*loaded);
                if self.shell.accounts != accounts { self.restart_engine(); }
                if self.toast.as_ref().is_none_or(|t| !t.error) { self.toast("settings reloaded"); }
            }
            JobDone::Connected { problems, tokens } => {
                if let Some((id, problem)) = problems.first() { self.error(format!("{id}: {problem}")); }
                self.shell.problems = problems.into_iter().collect();
                self.shell.tokens = tokens;
            }
        }
    }

    fn flow_event(&mut self, event: FlowEvent) {
        let providers = self.shell.services.providers();
        let effects = accounts_ui::step(&mut self.shell.flow, event, providers);
        for effect in effects {
            match effect {
                FlowEffect::Save { config, password } => {
                    let new = !self.shell.accounts.iter().any(|a| a.id == config.id);
                    let mut accounts: Vec<AccountConfig> = self.shell.accounts.iter().filter(|a| a.id != config.id).cloned().collect();
                    let at = self.shell.accounts.iter().position(|a| a.id == config.id).unwrap_or(accounts.len());
                    accounts.insert(at, config.clone());
                    self.spawn_job(Job::Save { config, password, accounts, new });
                }
                FlowEffect::SignIn(config) => {
                    self.toast("finish signing in in your browser · esc cancels");
                    self.spawn_job(Job::SignIn { config });
                }
                FlowEffect::CancelSignIn => self.cancel_sign_in(),
                FlowEffect::Closed { text, error } => if error { self.error(text) } else { self.toast(text) },
            }
        }
    }

    fn cancel_sign_in(&mut self) {
        if let Some(redirect) = self.shell.signin.take() {
            let _ = std::thread::Builder::new().name("ravenglass-job".into()).spawn(move || jobs::cancel_sign_in(&redirect));
        }
    }

    /// Keys the shell takes before the mode machine sees them: everything while locked or while
    /// the account form is up. `None`: the key is the mail UI's.
    pub fn shell_key(&mut self, press: KeyPress) -> Option<bool> {
        if self.shell.lock.concealed() {
            let verifier = self.shell.verifier();
            if let Asked::Verify(typed) = self.shell.lock.key(press, verifier.is_some(), self.clock) {
                match verifier {
                    Some(account) => self.spawn_job(Job::Verify { account, typed }),
                    None => self.shell.lock.verified(true, self.clock, false),
                }
            }
            return Some(true);
        }
        self.shell.lock.activity(self.clock);
        if self.shell.flow.is_some() {
            self.flow_event(FlowEvent::Key(press));
            return Some(true);
        }
        let idle = matches!(&self.mode, Mode::Normal { pending } if pending.is_empty());
        if self.shell.colors && idle && matches!(press.key, K::Esc | K::Char('q')) {
            self.shell.colors = false;
            return Some(true);
        }
        None
    }

    /// Each frame: answers from job threads, the lock's idle timer, the OS appearance.
    pub fn shell_poll(&mut self, clock: f64) -> bool {
        let done: Vec<JobDone> = self.shell.jobs_rx.try_iter().collect();
        let mut changed = !done.is_empty();
        for answer in done { self.fold_job(answer); }
        if self.shell.lock.tick(clock) {
            self.conceal();
            changed = true;
        }
        if let Some(due) = self.shell.lock.due() && self.shell.lock_wake != Some(due) {
            self.shell.lock_wake = Some(due);
            self.fx.push(Fx::WakeAt(due));
        }
        changed | self.follow_system()
    }

    /// The app just locked: nothing half-typed may be on screen when it opens again.
    fn conceal(&mut self) {
        // No fade: the content leaves the screen in the same frame the lock arrives.
        self.fx.push(Fx::Instant);
        self.mode = Mode::default();
        self.toast = None;
        if self.shell.flow.is_some() {
            self.cancel_sign_in();
            self.shell.flow = None;
        }
    }

    pub fn shell_cmd(&mut self, cmd: ShellCmd) {
        match cmd {
            ShellCmd::Lock => {
                self.shell.lock.lock_now();
                self.conceal();
            }
            ShellCmd::Colors => self.shell.colors = !self.shell.colors,
            ShellCmd::ThemeFixContrast => self.fix_contrast(),
            ShellCmd::ThemeSave(name) => self.save_theme_as(&name),
            ShellCmd::ThemeImport(path) => self.import_theme(path),
            ShellCmd::Reload => if self.shell.real { self.spawn_job(Job::Reload) } else { self.error("the demo has no settings to reload") },
            ShellCmd::AccountList => self.list_accounts(),
            ShellCmd::AccountAdd(email) => self.add_account(email.trim()),
            ShellCmd::AccountRemove(id) => self.remove_account(id.trim()),
            ShellCmd::AccountPassword(id) => self.reenter(id.trim(), false),
            ShellCmd::AccountSignIn(id) => self.reenter(id.trim(), true),
        }
    }

    fn needs_profile(&mut self) -> bool {
        if !self.shell.real { self.error("accounts need a profile: start without --demo"); }
        !self.shell.real
    }

    fn account(&self, id: &str) -> Option<AccountConfig> {
        self.shell.accounts.iter().find(|a| a.id.0.eq_ignore_ascii_case(id)).cloned()
    }

    fn list_accounts(&mut self) {
        if self.shell.accounts.is_empty() { return self.toast("no accounts · :account add you@example.com"); }
        let names: Vec<String> = self.shell.accounts.iter().map(|a| match self.shell.problems.get(&a.id) {
            Some(problem) => format!("{} ({})", a.id, problem.split(" · ").next().unwrap_or(problem)),
            None => a.id.0.clone(),
        }).collect();
        self.toast(format!("accounts: {}", names.join(", ")));
    }

    fn add_account(&mut self, email: &str) {
        if self.needs_profile() { return; }
        if autoconfig::domain(email).is_err() { return self.error("that is not an email address"); }
        if self.shell.accounts.iter().any(|a| a.address.eq_ignore_ascii_case(email)) { return self.error("that address already has an account"); }
        let taken: Vec<AccountId> = self.shell.accounts.iter().map(|a| a.id.clone()).collect();
        let id = accounts_ui::new_account_id(email, &taken);
        self.shell.colors = false;
        self.shell.flow = Some(Flow::Discovering { id, email: email.to_owned() });
        self.spawn_job(Job::Discover { email: email.to_owned() });
    }

    fn remove_account(&mut self, id: &str) {
        if self.needs_profile() { return; }
        let Some(account) = self.account(id) else { return self.error(format!("no account named {id}")) };
        let remaining: Vec<AccountConfig> = self.shell.accounts.iter().filter(|a| a.id != account.id).cloned().collect();
        // The engine goes first: the job stops it before it touches the store's file.
        let link = std::mem::replace(&mut self.link, Link::Off);
        self.shell.tokens = None;
        self.toast(format!("removing {}", account.id));
        self.spawn_job(Job::Remove { id: account.id, remaining, link });
    }

    /// `:account password <id>` and `:account signin <id>`: the secret of an account that is
    /// already set up, asked for again.
    fn reenter(&mut self, id: &str, browser: bool) {
        if self.needs_profile() { return; }
        let Some(account) = self.account(id) else { return self.error(format!("no account named {id}")) };
        let form = Form::from_config(&account);
        match (&account.auth, browser) {
            (AuthKind::Password, false) => self.shell.flow = Some(Flow::Password { form, input: Default::default(), existing: true }),
            (AuthKind::OAuth2(provider), true) => {
                let p = self.shell.services.providers();
                let has = match provider { mail_auth::oauth::Provider::Google => p.google, mail_auth::oauth::Provider::Microsoft => p.microsoft };
                if !has { return self.error(format!("this build has no {} OAuth client id ({})", accounts_ui::provider_name(*provider), accounts_ui::client_id_env(*provider))); }
                self.shell.flow = Some(Flow::SigningIn { form });
                self.toast("finish signing in in your browser · esc cancels");
                self.spawn_job(Job::SignIn { config: account });
            }
            (AuthKind::Password, true) => self.error(format!("{} signs in with a password: :account password {}", account.id, account.id)),
            (AuthKind::OAuth2(_), false) => self.error(format!("{} signs in through the browser: :account signin {}", account.id, account.id)),
        }
    }
}

/// The profile this run uses, or `None` for the demo. Headless runs are the demo unless they
/// name a profile, so scenes and scripts never touch the real one.
fn chosen_profile(options: &Options, env: crate::platform::Env, headless: bool) -> Option<Profile> {
    if options.demo { return None; }
    let named = options.profile.is_some() || env(PROFILE_ENV).is_some_and(|dir| !dir.trim().is_empty());
    if headless && !named { return None; }
    Profile::resolve(options.profile.as_deref(), env)
}

//! `ravenglass-relay`: run the Relay, or manage its users.
//!
//! ```text
//! ravenglass-relay [serve]            run, configured by the environment (docs/RELAY.md)
//! ravenglass-relay user add <name>    add a user; the password is read from standard input
//! ravenglass-relay user passwd <name> set a user's password, read from standard input
//! ravenglass-relay user remove <name> remove a user with all their accounts and secrets
//! ravenglass-relay user list          list users
//! ```
//!
//! No secret is ever a command-line argument: the token, the OIDC client secret and passwords
//! come from the environment, files or standard input.

use ravenglass_relay::config::{self, ModeSettings, OidcSettings, Settings, UsersSettings};
use ravenglass_relay::crypto::{self, MasterKey, PasswordCost};
use ravenglass_relay::store::Store;
use ravenglass_relay::{Relay, hub, oidc, server};
use std::sync::Arc;

const MIN_PASSWORD: usize = 10;

type Failure = Box<dyn std::error::Error>;

fn main() {
    if let Err(e) = run() {
        eprintln!("ravenglass-relay: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Failure> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let settings = config::load(&|name| std::env::var(name).ok())?;
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] | ["serve"] => serve(settings),
        ["user", rest @ ..] => manage_users(settings, rest),
        _ => Err("usage: ravenglass-relay [serve] | user add|passwd|remove <name> | user list".into()),
    }
}

fn users_settings(settings: &Settings) -> Result<&UsersSettings, Failure> {
    match &settings.mode { ModeSettings::Users(users) => Ok(users), ModeSettings::Token { .. } => Err("users exist only in users mode: set RAVENGLASS_RELAY_AUTH=users".into()) }
}

fn open_store(users: &UsersSettings) -> Result<Store, Failure> {
    let (master, created) = MasterKey::load_or_create(&users.master_key_file)?;
    if created { eprintln!("created a new master key at {}; back it up, without it no stored secret can be read", users.master_key_file.display()); }
    if crypto::readable_by_others(&users.master_key_file) { eprintln!("warning: {} can be read by other users; run chmod 600 on it", users.master_key_file.display()); }
    Ok(Store::open(&users.database, master)?)
}

fn read_password() -> Result<String, Failure> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']).to_owned();
    if password.chars().count() < MIN_PASSWORD { return Err(format!("the password must be at least {MIN_PASSWORD} characters").into()); }
    Ok(password)
}

fn manage_users(settings: Settings, args: &[&str]) -> Result<(), Failure> {
    let store = open_store(users_settings(&settings)?)?;
    match args {
        ["add", name] => {
            let password = read_password()?;
            store.add_user(name, Some(&crypto::hash_password(&password, PasswordCost::Standard)))?;
            eprintln!("added {name}");
        }
        ["passwd", name] => {
            let password = read_password()?;
            store.set_password(name, &crypto::hash_password(&password, PasswordCost::Standard))?;
            eprintln!("changed the password of {name}; their open sessions end when they expire or the relay restarts");
        }
        ["remove", name] => match store.remove_user(name)? {
            Some(_) => eprintln!("removed {name} and all their accounts"),
            None => return Err(format!("there is no user {name}").into()),
        },
        ["list"] => for user in store.users()? { println!("{}\t{}", user.id, user.name) },
        _ => return Err("usage: ravenglass-relay user add|passwd|remove <name> | user list".into()),
    }
    Ok(())
}

fn oidc_config(oidc: &OidcSettings) -> server::OidcConfig {
    server::OidcConfig {
        issuer: oidc.issuer.clone(),
        client: oidc::Client {
            client_id: oidc.client_id.clone(),
            client_secret: oidc.client_secret.clone(),
            redirect_uri: oidc.redirect_uri.clone(),
            scopes: oidc.scopes.clone(),
            allowed_emails: oidc.allowed_emails.clone(),
            allowed_domains: oidc.allowed_domains.clone(),
        },
    }
}

fn serve(settings: Settings) -> Result<(), Failure> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(&settings.listen).await?;
        match &settings.mode {
            ModeSettings::Token { token } => {
                let mut relay = Relay::new();
                if settings.demo { for backend in mail_mock::MockBackend::demo() { relay.add_backend(Arc::new(backend)); } }
                eprintln!("ravenglass-relay (token mode) listening on {}", listener.local_addr()?);
                server::serve(listener, relay, server::Config { token: token.clone(), origin: settings.origin.clone() }).await
            }
            ModeSettings::Users(users) => {
                let store = Arc::new(open_store(users).map_err(|e| std::io::Error::other(e.to_string()))?);
                let client_ids = ravenglass_accounts::ClientIds { google: users.google_client_id.clone(), microsoft: users.microsoft_client_id.clone() };
                let mut config = server::UsersConfig::new(store, hub::connector_opener(client_ids));
                config.origin = settings.origin.clone();
                config.password_login = users.password_login;
                config.oidc = users.oidc.as_ref().map(oidc_config);
                config.insecure_dev_cookies = users.insecure_dev_cookies;
                config.trust_forwarded_for = users.trust_forwarded_for;
                eprintln!("ravenglass-relay (users mode) listening on {}", listener.local_addr()?);
                server::serve_users(listener, config).await
            }
        }
    })?;
    Ok(())
}

//! Settings: environment variables, optionally on top of a TOML file named by
//! `RAVENGLASS_RELAY_CONFIG`. An environment variable always wins over the file. Every setting
//! is listed in `docs/RELAY.md`; the TOML keys are the variable names without the
//! `RAVENGLASS_RELAY_` prefix, in lowercase, with OIDC settings in an `[oidc]` table.
//!
//! Secrets (the shared token, the OIDC client secret) can come from a file instead
//! (`…_FILE`), so they need not sit in the environment. None is ever a command-line argument,
//! and [`Settings`] has no `Debug`, so it cannot be printed by accident.

use serde::Deserialize;
use std::path::PathBuf;

pub const PREFIX: &str = "RAVENGLASS_RELAY_";

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    listen: Option<String>,
    origin: Option<String>,
    auth: Option<String>,
    token: Option<String>,
    token_file: Option<String>,
    demo: Option<bool>,
    data_dir: Option<String>,
    database: Option<String>,
    master_key_file: Option<String>,
    password_login: Option<bool>,
    insecure_dev_cookies: Option<bool>,
    trust_forwarded_for: Option<bool>,
    public_url: Option<String>,
    google_client_id: Option<String>,
    microsoft_client_id: Option<String>,
    oidc: Option<OidcFile>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OidcFile {
    issuer: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    client_secret_file: Option<String>,
    scopes: Option<String>,
    allowed_emails: Option<Vec<String>>,
    allowed_domains: Option<Vec<String>>,
}

pub struct OidcSettings {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub scopes: String,
    pub allowed_emails: Vec<String>,
    pub allowed_domains: Vec<String>,
}

pub struct UsersSettings {
    pub database: PathBuf,
    pub master_key_file: PathBuf,
    pub password_login: bool,
    pub insecure_dev_cookies: bool,
    pub trust_forwarded_for: bool,
    pub oidc: Option<OidcSettings>,
    pub google_client_id: Option<String>,
    pub microsoft_client_id: Option<String>,
}

pub enum ModeSettings { Token { token: String }, Users(Box<UsersSettings>) }

pub struct Settings {
    pub listen: String,
    pub origin: Option<String>,
    pub demo: bool,
    /// Where users mode keeps its database and master key unless they are named separately.
    pub data_dir: PathBuf,
    pub mode: ModeSettings,
}

fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" | "" => Ok(false),
        _ => Err(format!("{PREFIX}{name} must be true or false")),
    }
}

fn list(value: &str) -> Vec<String> { value.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect() }

/// Read a secret from a file, trimming the trailing newline editors add. The error names the
/// file's setting, never its content.
fn read_secret(setting: &str, path: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {setting}: {}", e.kind()))?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

/// Load settings. `env` reads one environment variable (injected for tests).
pub fn load(env: &dyn Fn(&str) -> Option<String>) -> Result<Settings, String> {
    let var = |name: &str| env(&format!("{PREFIX}{name}")).filter(|v| !v.is_empty());
    let file: File = match var("CONFIG") {
        Some(path) => {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {PREFIX}CONFIG: {}", e.kind()))?;
            toml::from_str(&text).map_err(|e| format!("{PREFIX}CONFIG is not valid: {}", e.message()))?
        }
        None => File::default(),
    };
    let text = |name: &str, file: &Option<String>| var(name).or_else(|| file.clone().filter(|v| !v.is_empty()));
    let flag = |name: &str, file: Option<bool>, default: bool| -> Result<bool, String> {
        match var(name) { Some(v) => parse_bool(name, &v), None => Ok(file.unwrap_or(default)) }
    };
    let listen = text("LISTEN", &file.listen).unwrap_or_else(|| "127.0.0.1:8743".into());
    let origin = text("ORIGIN", &file.origin).map(|o| o.trim_end_matches('/').to_owned());
    let demo = flag("DEMO", file.demo, false)?;
    let data_dir = PathBuf::from(text("DATA_DIR", &file.data_dir).unwrap_or_else(|| "ravenglass-relay-data".into()));
    let mode = match text("AUTH", &file.auth).as_deref().unwrap_or("token") {
        "token" => {
            let token = match (text("TOKEN", &file.token), text("TOKEN_FILE", &file.token_file)) {
                (Some(token), _) => token,
                (None, Some(path)) => read_secret("TOKEN_FILE", &path)?,
                (None, None) => return Err(format!("token mode needs {PREFIX}TOKEN or {PREFIX}TOKEN_FILE")),
            };
            ModeSettings::Token { token }
        }
        "users" => {
            let oidc_file = file.oidc.unwrap_or_default();
            let oidc = match text("OIDC_ISSUER", &oidc_file.issuer) {
                None => None,
                Some(issuer) => {
                    let client_id = text("OIDC_CLIENT_ID", &oidc_file.client_id).ok_or(format!("OIDC needs {PREFIX}OIDC_CLIENT_ID"))?;
                    let client_secret = match (text("OIDC_CLIENT_SECRET", &oidc_file.client_secret), text("OIDC_CLIENT_SECRET_FILE", &oidc_file.client_secret_file)) {
                        (Some(secret), _) => Some(secret),
                        (None, Some(path)) => Some(read_secret("OIDC_CLIENT_SECRET_FILE", &path)?),
                        (None, None) => None,
                    };
                    let public = text("PUBLIC_URL", &file.public_url).ok_or(format!("OIDC needs {PREFIX}PUBLIC_URL, the Relay's own https URL"))?;
                    if !crate::oidc::secure_endpoint(&issuer) { return Err(format!("{PREFIX}OIDC_ISSUER must be https")); }
                    let allowed_emails = var("OIDC_ALLOWED_EMAILS").map(|v| list(&v)).or(oidc_file.allowed_emails.map(|v| list(&v.join(",")))).unwrap_or_default();
                    let allowed_domains = var("OIDC_ALLOWED_DOMAINS").map(|v| list(&v)).or(oidc_file.allowed_domains.map(|v| list(&v.join(",")))).unwrap_or_default();
                    Some(OidcSettings {
                        issuer,
                        client_id,
                        client_secret,
                        redirect_uri: format!("{}{}", public.trim_end_matches('/'), relay_proto::auth::OIDC_CALLBACK),
                        scopes: text("OIDC_SCOPES", &oidc_file.scopes).unwrap_or_else(|| "openid email profile".into()),
                        allowed_emails,
                        allowed_domains,
                    })
                }
            };
            let password_login = flag("PASSWORD_LOGIN", file.password_login, true)?;
            if !password_login && oidc.is_none() { return Err("users mode needs password sign-in or OIDC".into()); }
            ModeSettings::Users(Box::new(UsersSettings {
                database: text("DATABASE", &file.database).map_or_else(|| data_dir.join("relay.sqlite3"), PathBuf::from),
                master_key_file: text("MASTER_KEY_FILE", &file.master_key_file).map_or_else(|| data_dir.join("master.key"), PathBuf::from),
                password_login,
                insecure_dev_cookies: flag("INSECURE_DEV_COOKIES", file.insecure_dev_cookies, false)?,
                trust_forwarded_for: flag("TRUST_FORWARDED_FOR", file.trust_forwarded_for, false)?,
                oidc,
                google_client_id: env("RAVENGLASS_GOOGLE_CLIENT_ID").filter(|v| !v.is_empty()).or(file.google_client_id),
                microsoft_client_id: env("RAVENGLASS_MICROSOFT_CLIENT_ID").filter(|v| !v.is_empty()).or(file.microsoft_client_id),
            }))
        }
        other => return Err(format!("{PREFIX}AUTH must be token or users, not {other:?}")),
    };
    Ok(Settings { listen, origin, demo, data_dir, mode })
}

#[cfg(test)]
mod tests;

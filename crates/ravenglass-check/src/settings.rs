//! Which servers to talk to, how to log in, and the secret: autoconfig over real HTTPS, a look at
//! what was found, the chance to change it, and the password from a prompt without echo.

use crate::args::Args;
use crate::line;
use mail_auth::autoconfig::{self, Source};
use mail_auth::oauth::Provider;
use mail_auth::{AuthKind, Incoming};
use mail_imap::{Endpoint, Security};
use std::io::{BufRead, IsTerminal, Write};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Servers {
    Imap { imap: Endpoint, smtp: Endpoint },
    Jmap { session_url: String },
}

/// Everything needed to connect, except the secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub servers: Servers,
    pub login: String,
    /// Sign in through the browser with this provider.
    pub oauth: Option<Provider>,
    /// Where the settings came from, for the report.
    pub source: &'static str,
    /// The address's domain, masked wherever a host name contains it (empty for a known provider).
    pub domain: String,
    /// Extra trust anchors (DER) from `--ca`.
    pub roots: Vec<Vec<u8>>,
    /// `--smtp none`: submission is not checked.
    pub skip_smtp: bool,
}

pub enum Secret { Password(String), Token(String) }

/// Built-in providers, the provider's or Mozilla's autoconfig document (fetched over HTTPS), or
/// conventional names; then the command-line overrides, then the user's confirmation.
pub fn resolve(args: &Args) -> Result<Plan, String> {
    let domain = autoconfig::domain(&args.address).map_err(|e| e.to_string())?;
    let found = if args.jmap.is_some() || (args.imap.is_some() && (args.smtp.is_some() || args.no_smtp)) {
        None
    } else {
        Some(autoconfig::discover(&args.address, &|url: &str| ravenglass_accounts::http::fetch_document(url)).map_err(|e| e.to_string())?)
    };
    let source = match (&found, &args.jmap) {
        (_, Some(_)) => "command line (JMAP)",
        (None, None) => "command line",
        (Some(s), _) if s.source == Source::Builtin => "built in (known provider)",
        (Some(s), _) if s.source == Source::Ispdb => "autoconfig document (provider or Mozilla ISPDB)",
        _ => "guessed from the domain (nothing confirmed it)",
    };
    let servers = match (&args.jmap, &found) {
        (Some(url), _) => Servers::Jmap { session_url: url.clone() },
        (None, Some(s)) => match &s.incoming {
            Incoming::Jmap { session_url } if args.imap.is_none() => Servers::Jmap { session_url: session_url.clone() },
            Incoming::Imap(server) => Servers::Imap { imap: args.imap.clone().unwrap_or_else(|| endpoint(server)), smtp: args.smtp.clone().or_else(|| s.smtp.as_ref().map(endpoint)).unwrap_or_else(|| default_smtp(&domain)) },
            // A JMAP provider asked to use IMAP instead.
            Incoming::Jmap { .. } => Servers::Imap { imap: args.imap.clone().unwrap(), smtp: args.smtp.clone().unwrap_or_else(|| default_smtp(&domain)) },
        },
        (None, None) => Servers::Imap { imap: args.imap.clone().unwrap(), smtp: args.smtp.clone().unwrap_or_else(|| default_smtp(&domain)) },
    };
    let provider = found.as_ref().and_then(|s| match s.auth { AuthKind::OAuth2(p) => Some(p), AuthKind::Password => None });
    let oauth = if args.oauth { Some(provider.or_else(|| provider_for(&servers)).ok_or("OAuth is built in for Google and Microsoft accounts only")?) } else { None };
    let roots = match &args.ca {
        Some(path) => {
            let pem = std::fs::read_to_string(path).map_err(|e| format!("cannot read the --ca file: {}", e.kind()))?;
            let roots = mail_imap::certificates_from_pem(&pem);
            if roots.is_empty() { return Err("the --ca file holds no PEM certificate".into()); }
            roots
        }
        None => Vec::new(),
    };
    let login = args.login.clone().or_else(|| found.as_ref().map(|s| s.username.clone())).unwrap_or_else(|| args.address.trim().to_owned());
    // A known provider's hosts (imap.gmail.com) say nothing about the user: show them as they are.
    let domain = if found.as_ref().is_some_and(|s| s.source == Source::Builtin) { String::new() } else { domain };
    let mut plan = Plan { servers, login, oauth, source, domain, roots, skip_smtp: args.no_smtp };
    show(&plan, &args.address);
    if let Some(found) = &found && !matches!(found.auth, AuthKind::Password) && plan.oauth.is_none() {
        line("note", "this provider signs in with OAuth; with a password expect an app password to be required (see docs/CHECKING-ACCOUNTS.md), or pass --oauth");
    }
    if let Servers::Jmap { session_url } = &plan.servers && session_url.contains("fastmail.com") && !args.token && plan.oauth.is_none() {
        line("note", "Fastmail's JMAP takes an API token (pass --token), not an app password; for IMAP with an app password use --imap imap.fastmail.com:993 --smtp smtp.fastmail.com:465");
    }
    if !args.yes {
        if !std::io::stdin().is_terminal() { return Err("confirm the settings interactively, or pass --yes".into()); }
        confirm(&mut plan, &args.address)?;
    }
    Ok(plan)
}

fn endpoint(server: &mail_auth::Server) -> Endpoint {
    let security = match server.security { mail_auth::Security::Tls => Security::Tls, mail_auth::Security::StartTls => Security::StartTls, mail_auth::Security::Plain => Security::Plain };
    Endpoint::new(&server.host, server.port, security)
}

fn default_smtp(domain: &str) -> Endpoint { Endpoint::new(&format!("smtp.{domain}"), 465, Security::Tls) }

fn provider_for(servers: &Servers) -> Option<Provider> {
    let Servers::Imap { imap, .. } = servers else { return None };
    if imap.host.ends_with("gmail.com") || imap.host.ends_with("googlemail.com") { return Some(Provider::Google); }
    if imap.host.ends_with("office365.com") || imap.host.ends_with("outlook.com") { return Some(Provider::Microsoft); }
    None
}

/// A host name for the report: the user's own domain is masked, a provider's host is not.
pub fn host_label(host: &str, domain: &str) -> String {
    let host = host.to_ascii_lowercase();
    if domain.is_empty() { return host; }
    if host == domain { return "<your domain>".into(); }
    match host.strip_suffix(domain).filter(|rest| rest.ends_with('.')) {
        Some(rest) => format!("{rest}<your domain>"),
        None => host,
    }
}

/// `host:port security` with the domain masked.
pub fn describe(endpoint: &Endpoint, domain: &str) -> String {
    let security = match endpoint.security { Security::Tls => "TLS", Security::StartTls => "STARTTLS", Security::Plain => "no encryption" };
    format!("{}:{} {security}", host_label(&endpoint.host, domain), endpoint.port)
}

/// A URL for the report: the host masked as above, the path kept.
pub fn url_label(url: &str, domain: &str) -> String {
    let Some(rest) = url.strip_prefix("https://") else { return "<not https>".into() };
    let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    let (name, port) = host.split_once(':').map_or((host, None), |(n, p)| (n, Some(p)));
    format!("https://{}{}/{path}", host_label(name, domain), port.map(|p| format!(":{p}")).unwrap_or_default())
}

fn show(plan: &Plan, address: &str) {
    line("settings", format!("from {}", plan.source));
    match &plan.servers {
        Servers::Imap { imap, smtp } => {
            line("", format!("IMAP {}", describe(imap, &plan.domain)));
            if plan.skip_smtp { line("", "SMTP not checked"); } else { line("", format!("SMTP {}", describe(smtp, &plan.domain))); }
        }
        Servers::Jmap { session_url } => line("", format!("JMAP {}", url_label(session_url, &plan.domain))),
    }
    let login = if plan.login.eq_ignore_ascii_case(address.trim()) { "the address".to_owned() } else { format!("another name ({} characters)", plan.login.chars().count()) };
    let auth = match plan.oauth { Some(p) => format!("OAuth ({p:?})"), None => "password".into() };
    line("", format!("login {login}, sign-in {auth}{}", if plan.roots.is_empty() { "" } else { ", trusting the --ca certificate" }));
}

fn ask(question: &str) -> Result<String, String> {
    print!("{question}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(answer.trim().to_owned())
}

/// Use, change or give up on the settings shown.
fn confirm(plan: &mut Plan, address: &str) -> Result<(), String> {
    loop {
        match ask("Use these settings? [Y]es / [e]dit / [q]uit: ")?.to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(()),
            "q" | "quit" | "n" | "no" => return Err("stopped".into()),
            "e" | "edit" => { edit(plan)?; show(plan, address); }
            _ => {}
        }
    }
}

fn edit(plan: &mut Plan) -> Result<(), String> {
    println!("Press Enter to keep a value. Servers are host:port[:tls|starttls].");
    match &mut plan.servers {
        Servers::Imap { imap, smtp } => {
            let typed = ask(&format!("IMAP [{}:{}]: ", imap.host, imap.port))?;
            if !typed.is_empty() { *imap = crate::args::endpoint(&typed, 993)?; }
            let typed = ask(&format!("SMTP [{}:{}]: ", smtp.host, smtp.port))?;
            if !typed.is_empty() { *smtp = crate::args::endpoint(&typed, 465)?; }
        }
        Servers::Jmap { session_url } => {
            let typed = ask(&format!("JMAP session URL [{session_url}]: "))?;
            if !typed.is_empty() {
                if !typed.starts_with("https://") { return Err("the JMAP session URL must start with https://".into()); }
                *session_url = typed;
            }
        }
    }
    // The login is not echoed back on screen (it is often the address); type a new one or keep it.
    let typed = ask("Login name [keep]: ")?;
    if !typed.is_empty() { plan.login = typed; }
    plan.source = "edited by hand";
    Ok(())
}

/// The password, app password or token: from `RAVENGLASS_CHECK_PASSWORD`, else a prompt that does
/// not echo. With `--oauth`, the browser sign-in instead.
pub fn secret(args: &Args, plan: &Plan) -> Result<Secret, String> {
    if let Some(provider) = plan.oauth { return crate::oauth::sign_in(provider).map(Secret::Token); }
    let what = if args.token { "API token" } else { "password (or app password)" };
    let value = match std::env::var("RAVENGLASS_CHECK_PASSWORD") {
        Ok(value) if !value.is_empty() => { line("login", format!("{what} from RAVENGLASS_CHECK_PASSWORD")); value }
        _ => {
            if !std::io::stdin().is_terminal() { return Err(format!("no terminal to ask for the {what}; set RAVENGLASS_CHECK_PASSWORD")); }
            rpassword::prompt_password(format!("{what} (not shown): ")).map_err(|e| format!("could not read the {what}: {}", e.kind()))?
        }
    };
    if value.is_empty() { return Err(format!("no {what} given")); }
    Ok(if args.token { Secret::Token(value) } else { Secret::Password(value) })
}

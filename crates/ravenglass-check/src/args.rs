//! The command line. Secrets never come from here: a password on the command line ends up in
//! shell history and in the process list.

use mail_imap::{Endpoint, Security};
use std::path::PathBuf;

pub const USAGE: &str = "\
usage: ravenglass-check [options] <address>

Checks one mail account with Ravenglass's own IMAP/SMTP or JMAP code. Read-only by default.
Every line it prints is safe to paste into a bug report: no addresses, logins, passwords,
subjects, previews, file names or names of your own folders.

The password (or app password, or API token) is read without echo, or from the environment
variable RAVENGLASS_CHECK_PASSWORD. It is never taken as an argument.

options:
  --jmap <session-url>    use JMAP at this session URL (e.g. https://api.fastmail.com/jmap/session)
  --token                 the secret is an API token sent as a Bearer token (Fastmail JMAP)
  --oauth                 sign in with OAuth in the browser (Gmail, Microsoft 365); needs
                          RAVENGLASS_GOOGLE_CLIENT_ID or RAVENGLASS_MICROSOFT_CLIENT_ID
  --imap <host:port[:tls|starttls]>   IMAP server, instead of looking it up
  --smtp <host:port[:tls|starttls]>   SMTP submission server, instead of looking it up
                          (`--smtp none`: skip the submission check)
  --login <name>          log in with this name instead of the address
  --ca <file.pem>         trust this certificate (a private CA) instead of the public roots
  --yes                   use the settings found without asking
  --idle <seconds>        how long to watch the inbox for push (default 30, 0 to skip)
  --limit <count>         how many of the newest inbox messages to read (default 200)
  --write                 also check changes, only inside a folder it creates and deletes
                          (Ravenglass-Check); nothing else in the account is touched
  -h, --help              this text
";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub address: String,
    pub jmap: Option<String>,
    pub token: bool,
    pub oauth: bool,
    pub imap: Option<Endpoint>,
    pub smtp: Option<Endpoint>,
    /// `--smtp none`: an IMAP-only check.
    pub no_smtp: bool,
    pub login: Option<String>,
    pub ca: Option<PathBuf>,
    pub yes: bool,
    pub idle_secs: u64,
    pub limit: u32,
    pub write: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed { Run(Args), Help }

pub fn parse(mut words: impl Iterator<Item = String>) -> Result<Parsed, String> {
    let mut args = Args { idle_secs: 30, limit: 200, ..Args::default() };
    let mut address = None;
    while let Some(word) = words.next() {
        let mut value = |name: &str| words.next().ok_or_else(|| format!("{name} needs a value"));
        match word.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--jmap" => args.jmap = Some(value("--jmap")?),
            "--token" => args.token = true,
            "--oauth" => args.oauth = true,
            "--imap" => args.imap = Some(endpoint(&value("--imap")?, 993)?),
            "--smtp" => match value("--smtp")?.as_str() {
                "none" => args.no_smtp = true,
                given => args.smtp = Some(endpoint(given, 465)?),
            },
            "--login" => args.login = Some(value("--login")?),
            "--ca" => args.ca = Some(PathBuf::from(value("--ca")?)),
            "--yes" | "-y" => args.yes = true,
            "--idle" => args.idle_secs = value("--idle")?.parse().map_err(|_| "--idle takes a number of seconds".to_owned())?,
            "--limit" => args.limit = value("--limit")?.parse().ok().filter(|n| (1..=10_000).contains(n)).ok_or("--limit takes a number from 1 to 10000")?,
            "--write" => args.write = true,
            "--password" | "-p" => return Err("passwords are never taken as arguments; use the prompt or RAVENGLASS_CHECK_PASSWORD".into()),
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            _ if address.is_some() => return Err("give one address".into()),
            _ => address = Some(word),
        }
    }
    args.address = address.ok_or("which address? e.g. ravenglass-check you@example.com")?;
    mail_auth::autoconfig::domain(&args.address).map_err(|_| "that is not an email address".to_owned())?;
    if let Some(url) = &args.jmap && !url.starts_with("https://") { return Err("the JMAP session URL must start with https://".into()); }
    if args.token && args.oauth { return Err("--token and --oauth are two different ways to sign in; pick one".into()); }
    Ok(Parsed::Run(args))
}

/// `host:port[:tls|starttls]`. The security defaults to TLS, or STARTTLS on the ports that use it.
pub fn endpoint(text: &str, default_port: u16) -> Result<Endpoint, String> {
    let mut parts = text.split(':');
    let host = parts.next().filter(|h| !h.is_empty()).ok_or("a server needs a host name")?;
    let port: u16 = match parts.next() { Some(p) => p.parse().map_err(|_| format!("{p} is not a port"))?, None => default_port };
    let security = match parts.next() {
        Some("tls" | "ssl") => Security::Tls,
        Some("starttls") => Security::StartTls,
        Some(other) => return Err(format!("{other}: use tls or starttls (unencrypted connections are not offered)")),
        None if matches!(port, 143 | 587 | 25) => Security::StartTls,
        None => Security::Tls,
    };
    if parts.next().is_some() { return Err(format!("{text}: expected host:port[:tls|starttls]")); }
    Ok(Endpoint::new(host, port, security))
}

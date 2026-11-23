//! Server settings from an email address (architecture §4 "Any IMAP/SMTP host"). Three sources
//! in order: providers we know, the provider's or Mozilla's ISPDB document, then conventional
//! host names for the user to confirm. Pure: [`discover`] takes the HTTP fetch as a function.

use crate::config::{AccountConfig, AuthKind, Incoming, Security, Server};
use crate::oauth::Provider;
use mail_core::AccountId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Builtin,
    Ispdb,
    /// Host names by convention; nothing confirmed them. Show them to the user before use.
    Guess,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings { pub incoming: Incoming, pub smtp: Option<Server>, pub username: String, pub auth: AuthKind, pub server_saves_sent: bool, pub source: Source }

impl Settings {
    pub fn into_account(self, id: AccountId, name: &str, address: &str) -> AccountConfig {
        AccountConfig { id, name: name.to_owned(), address: address.to_owned(), username: self.username, incoming: self.incoming, smtp: self.smtp, auth: self.auth, server_saves_sent: self.server_saves_sent }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AutoconfigError {
    #[error("not an email address")]
    NotAnAddress,
    #[error("the settings document is not valid: {0}")]
    BadDocument(String),
    #[error("the settings document names no IMAP server")]
    NoImap,
}

/// The domain of an address, lower-cased.
pub fn domain(email: &str) -> Result<String, AutoconfigError> {
    match email.trim().rsplit_once('@') {
        Some((local, domain)) if !local.is_empty() && domain.contains('.') && !domain.contains(char::is_whitespace) => Ok(domain.to_ascii_lowercase()),
        _ => Err(AutoconfigError::NotAnAddress),
    }
}

fn imap_smtp(imap: &str, smtp: &str, smtp_port: u16, smtp_security: Security) -> (Incoming, Option<Server>) {
    (Incoming::Imap(Server::new(imap, 993, Security::Tls)), Some(Server::new(smtp, smtp_port, smtp_security)))
}

/// Providers whose settings are stable and public.
pub fn builtin(email: &str) -> Option<Settings> {
    let domain = domain(email).ok()?;
    let settings = |(incoming, smtp): (Incoming, Option<Server>), auth, server_saves_sent| Settings { incoming, smtp, username: email.trim().to_owned(), auth, server_saves_sent, source: Source::Builtin };
    Some(match domain.as_str() {
        "gmail.com" | "googlemail.com" => settings(imap_smtp("imap.gmail.com", "smtp.gmail.com", 465, Security::Tls), AuthKind::OAuth2(Provider::Google), true),
        "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => settings(imap_smtp("outlook.office365.com", "smtp-mail.outlook.com", 587, Security::StartTls), AuthKind::OAuth2(Provider::Microsoft), true),
        "icloud.com" | "me.com" | "mac.com" => settings(imap_smtp("imap.mail.me.com", "smtp.mail.me.com", 587, Security::StartTls), AuthKind::Password, false),
        "fastmail.com" | "fastmail.fm" => settings((Incoming::Jmap { session_url: "https://api.fastmail.com/jmap/session".into() }, None), AuthKind::Password, true),
        "yahoo.com" | "ymail.com" => settings(imap_smtp("imap.mail.yahoo.com", "smtp.mail.yahoo.com", 465, Security::Tls), AuthKind::Password, false),
        _ => return None,
    })
}

/// Where a domain's ISPDB document may live, most specific first. Only the provider's own host
/// is sent the address; Mozilla's database is asked by domain alone.
pub fn ispdb_urls(email: &str) -> Result<Vec<String>, AutoconfigError> {
    let d = domain(email)?;
    let address = crate::oauth::form_encode(email.trim());
    Ok(vec![
        format!("https://autoconfig.{d}/mail/config-v1.1.xml?emailaddress={address}"),
        format!("https://{d}/.well-known/autoconfig/mail/config-v1.1.xml"),
        format!("https://autoconfig.thunderbird.net/v1.1/{d}"),
    ])
}

/// Parse a Mozilla autoconfig document (`config-v1.1.xml`).
pub fn parse_ispdb(xml: &str, email: &str) -> Result<Settings, AutoconfigError> {
    let d = domain(email)?;
    let doc = roxmltree::Document::parse(xml).map_err(|e| AutoconfigError::BadDocument(e.to_string()))?;
    let local = email.trim().rsplit_once('@').map_or("", |(l, _)| l);
    let fill = |s: &str| s.replace("%EMAILADDRESS%", email.trim()).replace("%EMAILLOCALPART%", local).replace("%EMAILDOMAIN%", &d);
    let server = |tag: &str, kind: &str| {
        doc.descendants().filter(|n| n.has_tag_name(tag) && n.attribute("type") == Some(kind)).find_map(|n| {
            let child = |name: &str| n.children().find(|c| c.has_tag_name(name)).and_then(|c| c.text()).map(str::trim);
            let security = match child("socketType")? { "SSL" => Security::Tls, "STARTTLS" => Security::StartTls, _ => return None };
            let oauth = n.children().any(|c| c.has_tag_name("authentication") && c.text().map(str::trim) == Some("OAuth2"));
            Some((Server { host: fill(child("hostname")?), port: child("port")?.parse().ok()?, security }, fill(child("username").unwrap_or("%EMAILADDRESS%")), oauth))
        })
    };
    let (imap, username, _) = server("incomingServer", "imap").ok_or(AutoconfigError::NoImap)?;
    let smtp = server("outgoingServer", "smtp").map(|(s, _, _)| s);
    // OAuth needs a registered client, which only the built-in providers have.
    Ok(Settings { incoming: Incoming::Imap(imap), smtp, username, auth: AuthKind::Password, server_saves_sent: false, source: Source::Ispdb })
}

/// Conventional names for a custom domain, to pre-fill the manual form.
pub fn guess(email: &str) -> Result<Settings, AutoconfigError> {
    let d = domain(email)?;
    let (incoming, smtp) = imap_smtp(&format!("imap.{d}"), &format!("smtp.{d}"), 465, Security::Tls);
    Ok(Settings { incoming, smtp, username: email.trim().to_owned(), auth: AuthKind::Password, server_saves_sent: false, source: Source::Guess })
}

/// Built-in, then each ISPDB location through `fetch` (which returns a document body or
/// `None`), then a guess. A document that does not parse is skipped, not fatal.
pub fn discover(email: &str, fetch: &dyn Fn(&str) -> Option<String>) -> Result<Settings, AutoconfigError> {
    if let Some(known) = builtin(email) { return Ok(known); }
    for url in ispdb_urls(email)? {
        if let Some(found) = fetch(&url).and_then(|xml| parse_ispdb(&xml, email).ok()) { return Ok(found); }
    }
    guess(email)
}

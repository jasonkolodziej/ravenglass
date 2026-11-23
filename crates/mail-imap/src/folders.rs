//! Turning a LIST response into folders: roles from SPECIAL-USE attributes (RFC 6154) with
//! well-known names as the fallback, display names decoded from modified UTF-7.

use crate::ids::folder_id;
use async_imap::types::NameAttribute;
use mail_core::{AccountId, Folder, FolderRole};

/// One selectable mailbox from LIST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// The name as the server spells it (still modified UTF-7).
    pub mailbox: String,
    pub delimiter: Option<String>,
    /// The role its SPECIAL-USE attribute gives it, if any.
    pub special_use: Option<FolderRole>,
}

/// `None` for a mailbox that cannot be selected, which is not a folder to the user.
pub fn listed(mailbox: &str, delimiter: Option<&str>, attributes: &[NameAttribute<'_>]) -> Option<Listed> {
    let mut special_use = None;
    for attribute in attributes {
        let role = match attribute {
            NameAttribute::NoSelect => return None,
            NameAttribute::Extension(other) if other.eq_ignore_ascii_case("\\NonExistent") => return None,
            NameAttribute::Drafts => FolderRole::Drafts,
            NameAttribute::Sent => FolderRole::Sent,
            NameAttribute::Trash => FolderRole::Trash,
            NameAttribute::Junk => FolderRole::Junk,
            NameAttribute::Archive => FolderRole::Archive,
            NameAttribute::Flagged => FolderRole::Flagged,
            // Gmail's "All Mail": moving a message there is what archiving means on Gmail.
            NameAttribute::All => FolderRole::Archive,
            _ => continue,
        };
        // `\Archive` beats `\All` if a server ever gives both.
        if special_use.is_none() || matches!(attribute, NameAttribute::Archive) { special_use = Some(role); }
    }
    Some(Listed { mailbox: mailbox.to_owned(), delimiter: delimiter.map(str::to_owned), special_use })
}

/// The role a folder's own name suggests, for servers without SPECIAL-USE.
pub fn role_from_name(leaf: &str) -> FolderRole {
    match leaf.trim().to_lowercase().as_str() {
        "inbox" => FolderRole::Inbox,
        "drafts" | "draft" => FolderRole::Drafts,
        "sent" | "sent items" | "sent mail" | "sent messages" => FolderRole::Sent,
        "trash" | "deleted items" | "deleted messages" | "bin" => FolderRole::Trash,
        "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" => FolderRole::Junk,
        "archive" | "archives" | "all mail" => FolderRole::Archive,
        "flagged" | "starred" => FolderRole::Flagged,
        _ => FolderRole::Other,
    }
}

/// One role per mailbox, in LIST order. Each role but `Other` goes to at most one folder: the
/// server's attribute wins, then a top-level name match, then a nested one.
pub fn assign_roles(listed: &[Listed]) -> Vec<FolderRole> {
    let mut roles = vec![FolderRole::Other; listed.len()];
    let taken = |roles: &[FolderRole], role| roles.contains(&role);
    for (i, l) in listed.iter().enumerate() {
        if l.mailbox.eq_ignore_ascii_case("INBOX") { roles[i] = FolderRole::Inbox; }
    }
    for (i, l) in listed.iter().enumerate() {
        if let Some(role) = l.special_use && roles[i] == FolderRole::Other && !taken(&roles, role) { roles[i] = role; }
    }
    for nested in [false, true] {
        for (i, l) in listed.iter().enumerate() {
            let path = path_of(l);
            let top_level = path.len() == 1 || (path.len() == 2 && is_namespace(&path[0]));
            let role = role_from_name(path.last().map_or("", String::as_str));
            if roles[i] == FolderRole::Other && role != FolderRole::Other && role != FolderRole::Inbox && top_level != nested && !taken(&roles, role) { roles[i] = role; }
        }
    }
    roles
}

/// Prefixes that hold an account's standard folders without being one (`INBOX.Sent`, `[Gmail]/Drafts`).
fn is_namespace(segment: &str) -> bool { segment.eq_ignore_ascii_case("INBOX") || segment.starts_with('[') }

/// Decoded path segments, e.g. `["Receipts", "2026"]`.
pub fn path_of(listed: &Listed) -> Vec<String> {
    let name = decode_utf7(&listed.mailbox);
    match listed.delimiter.as_deref().filter(|d| !d.is_empty()) {
        Some(delimiter) => name.split(delimiter).filter(|s| !s.is_empty()).map(str::to_owned).collect(),
        None => vec![name],
    }
}

/// Counts are filled in by the caller from STATUS.
pub fn to_folder(account: &AccountId, listed: &Listed, role: FolderRole) -> Folder {
    let path = path_of(listed);
    let name = if role == FolderRole::Inbox { "Inbox".to_owned() } else { path.last().cloned().unwrap_or_else(|| listed.mailbox.clone()) };
    Folder { id: folder_id(account, &listed.mailbox), account: account.clone(), name, path, role, unread: 0, total: 0 }
}

/// Where to create a missing Drafts or Sent folder: beside the others, under `INBOX.` on servers
/// that keep everything there.
pub fn name_for_new(listed: &[Listed], leaf: &str) -> String {
    let delimiter = listed.iter().find_map(|l| l.delimiter.clone()).unwrap_or_default();
    let prefix = format!("INBOX{delimiter}");
    let others = || listed.iter().filter(|l| !l.mailbox.eq_ignore_ascii_case("INBOX"));
    if !delimiter.is_empty() && others().next().is_some() && others().all(|l| l.mailbox.starts_with(&prefix)) { format!("{prefix}{leaf}") } else { leaf.to_owned() }
}

/// Where to create `leaf` when the server refuses it at the top level: under INBOX, on servers
/// whose personal namespace is `INBOX.` (Courier, Dovecot set up that way). An account there that
/// holds only an inbox gives no other hint; the NAMESPACE reply would, but async-imap cannot read it.
pub fn under_inbox(listed: &[Listed], leaf: &str) -> Option<String> {
    let inbox = listed.iter().find(|l| l.mailbox.eq_ignore_ascii_case("INBOX"))?;
    let delimiter = inbox.delimiter.as_deref().filter(|d| !d.is_empty())?;
    Some(format!("{}{delimiter}{leaf}", inbox.mailbox))
}

/// Modified UTF-7 (RFC 3501 §5.1.3) to text. Anything malformed is returned as it came.
pub fn decode_utf7(name: &str) -> String {
    use base64::Engine;
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let Some(len) = rest[start + 1..].find('-') else { return name.to_owned() };
        let encoded = &rest[start + 1..start + 1 + len];
        if encoded.is_empty() {
            out.push('&');
        } else {
            let Ok(bytes) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(encoded.replace(',', "/")) else { return name.to_owned() };
            if bytes.len() % 2 != 0 { return name.to_owned(); }
            let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect();
            let Ok(text) = String::from_utf16(&units) else { return name.to_owned() };
            out.push_str(&text);
        }
        rest = &rest[start + 1 + len + 1..];
    }
    out.push_str(rest);
    out
}

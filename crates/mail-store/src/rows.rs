//! Row ↔ type mapping for the `messages`, `folders` and `search` tables.

use crate::Result;
use mail_core::*;
use rusqlite::{Row, Transaction, params};
use serde::{Deserialize, Serialize};

pub const ENVELOPE_COLUMNS: &str =
    "m.id, m.folder, m.thread, m.from_name, m.from_email, m.recipients, m.subject, m.preview, m.date, m.flags, m.attachments";

#[derive(Serialize, Deserialize)]
struct Recipients { to: Vec<Address>, cc: Vec<Address> }

const ROLES: [(FolderRole, &str); 8] = [
    (FolderRole::Inbox, "inbox"), (FolderRole::Drafts, "drafts"), (FolderRole::Sent, "sent"), (FolderRole::Archive, "archive"),
    (FolderRole::Trash, "trash"), (FolderRole::Junk, "junk"), (FolderRole::Flagged, "flagged"), (FolderRole::Other, "other"),
];

pub fn role_name(role: FolderRole) -> &'static str { ROLES.iter().find(|(r, _)| *r == role).map_or("other", |(_, n)| n) }

fn role_from(name: &str) -> FolderRole { ROLES.iter().find(|(_, n)| *n == name).map_or(FolderRole::Other, |(r, _)| *r) }

/// The inner `Result` carries JSON errors, which `rusqlite`'s row callback cannot.
pub fn folder(r: &Row<'_>) -> rusqlite::Result<Result<Folder>> {
    let (path, role): (String, String) = (r.get(3)?, r.get(4)?);
    let (id, account, name, unread, total) = (FolderId(r.get(0)?), AccountId(r.get(1)?), r.get(2)?, r.get(5)?, r.get(6)?);
    Ok(serde_json::from_str(&path).map(|path| Folder { id, account, name, path, role: role_from(&role), unread, total }).map_err(Into::into))
}

pub fn envelope(r: &Row<'_>) -> rusqlite::Result<Result<Envelope>> {
    let (recipients, attachments): (String, String) = (r.get(5)?, r.get(10)?);
    let (id, folder, thread) = (MessageId(r.get(0)?), FolderId(r.get(1)?), ThreadId(r.get(2)?));
    let from = Address { name: r.get(3)?, email: r.get(4)? };
    let (subject, preview, date, flags) = (r.get(6)?, r.get(7)?, r.get(8)?, Flags(r.get(9)?));
    let parsed = || -> Result<Envelope> {
        let Recipients { to, cc } = serde_json::from_str(&recipients)?;
        Ok(Envelope { id, folder, thread, from, to, cc, subject, preview, date, flags, attachments: serde_json::from_str(&attachments)? })
    };
    Ok(parsed())
}

fn address_text(a: &Address) -> String { format!("{} {}", a.name.as_deref().unwrap_or(""), a.email) }

pub fn upsert_envelope(tx: &Transaction<'_>, account: &AccountId, e: &Envelope) -> Result<()> {
    let recipients = serde_json::to_string(&Recipients { to: e.to.clone(), cc: e.cc.clone() })?;
    let existing: Option<i64> = {
        use rusqlite::OptionalExtension;
        tx.query_row("SELECT rowid FROM messages WHERE id = ?1", [&e.id.0], |r| r.get(0)).optional()?
    };
    let values = params![e.id.0, account.0, e.folder.0, e.thread.0, e.from.name, e.from.email, recipients, e.subject, e.preview, e.date, e.flags.0, serde_json::to_string(&e.attachments)?];
    let rowid = match existing {
        Some(rowid) => {
            tx.execute(
                "UPDATE messages SET account = ?2, folder = ?3, thread = ?4, from_name = ?5, from_email = ?6, recipients = ?7,
                 subject = ?8, preview = ?9, date = ?10, flags = ?11, attachments = ?12 WHERE id = ?1",
                values,
            )?;
            rowid
        }
        None => {
            tx.execute(
                "INSERT INTO messages (id, account, folder, thread, from_name, from_email, recipients, subject, preview, date, flags, attachments)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                values,
            )?;
            tx.last_insert_rowid()
        }
    };
    let to_text = e.to.iter().chain(&e.cc).map(address_text).collect::<Vec<_>>().join(" ");
    if existing.is_some() {
        // Keep the indexed body text of a message we already had.
        tx.execute("UPDATE search SET sender = ?2, recipients = ?3, subject = ?4, preview = ?5 WHERE rowid = ?1", params![rowid, address_text(&e.from), to_text, e.subject, e.preview])?;
    } else {
        tx.execute("INSERT INTO search (rowid, sender, recipients, subject, preview, body) VALUES (?1, ?2, ?3, ?4, ?5, '')", params![rowid, address_text(&e.from), to_text, e.subject, e.preview])?;
        tx.execute(
            "INSERT INTO contacts (email, name) VALUES (?1, ?2) ON CONFLICT (email) DO UPDATE SET uses = uses + 1, name = coalesce(excluded.name, name)",
            params![e.from.email.to_ascii_lowercase(), e.from.name],
        )?;
    }
    Ok(())
}

pub fn delete_message(tx: &Transaction<'_>, id: &MessageId) -> Result<()> {
    tx.execute("DELETE FROM search WHERE rowid = (SELECT rowid FROM messages WHERE id = ?1)", [&id.0])?;
    tx.execute("DELETE FROM bodies WHERE message_id = ?1", [&id.0])?;
    tx.execute("DELETE FROM messages WHERE id = ?1", [&id.0])?;
    Ok(())
}

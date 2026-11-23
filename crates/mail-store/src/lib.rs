//! The local cache (architecture §6): one SQLite schema for every client, with FTS5 search.
//!
//! The API takes no `Path`: a store opens from a location string that the platform adapter picks
//! (a file on desktop, an OPFS name on the web), so shared code stays free of `std::fs`.
//! Native builds use `rusqlite` with bundled SQLite. The web build (W1) is expected to keep this
//! API on SQLite compiled to wasm; that target is not built or tested yet.

mod query;
mod rows;
#[cfg(test)]
mod tests;

pub use query::{Query, Token, parse_date};

use mail_core::*;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("stored JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("database schema v{found} is newer than this build (v{supported})")]
    TooNew { found: u32, supported: u32 },
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Forward-only migrations, in order. `user_version` records how many have run.
const MIGRATIONS: &[&str] = &[include_str!("migrations/0001_init.sql")];

/// Default cap on cached raw bodies (desktop: 2 GB; the web build passes 300 MB).
pub const DEFAULT_BODY_BYTES: u64 = 2 << 30;

pub struct Store {
    conn: Connection,
    body_bytes_cap: u64,
}

impl Store {
    pub fn open_in_memory() -> Result<Self> { Self::init(Connection::open_in_memory()?) }

    /// `location` is whatever the platform adapter resolved; on desktop, a file path.
    pub fn open(location: &str) -> Result<Self> {
        let conn = Connection::open(location)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        let found: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let supported = MIGRATIONS.len() as u32;
        if found > supported { return Err(StoreError::TooNew { found, supported }); }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as u32 + 1)?;
            tx.commit()?;
        }
        Ok(Self { conn, body_bytes_cap: DEFAULT_BODY_BYTES })
    }

    pub fn schema_version(&self) -> Result<u32> { Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?) }

    pub fn set_body_cache_bytes(&mut self, cap: u64) { self.body_bytes_cap = cap; }

    // ── folders ────────────────────────────────────────────────────────────────────────────

    /// Insert or update folders from the server's list; sync tokens are kept.
    pub fn upsert_folders(&mut self, folders: &[Folder]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for f in folders {
            tx.execute("INSERT OR IGNORE INTO accounts (id) VALUES (?1)", [&f.account.0])?;
            tx.execute(
                "INSERT INTO folders (id, account, name, path, role) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (id) DO UPDATE SET name = ?3, path = ?4, role = ?5",
                params![f.id.0, f.account.0, f.name, serde_json::to_string(&f.path)?, rows::role_name(f.role)],
            )?;
        }
        Ok(tx.commit()?)
    }

    /// Folders with counts from the local messages, so optimistic changes show at once.
    pub fn folders(&self, account: &AccountId) -> Result<Vec<Folder>> {
        let mut q = self.conn.prepare_cached(
            "SELECT f.id, f.account, f.name, f.path, f.role,
                    (SELECT count(*) FROM messages m WHERE m.folder = f.id AND m.flags & 1 = 0),
                    (SELECT count(*) FROM messages m WHERE m.folder = f.id)
             FROM folders f WHERE f.account = ?1 ORDER BY f.rowid",
        )?;
        let found = q.query_map([&account.0], rows::folder)?.collect::<rusqlite::Result<Vec<_>>>()?;
        found.into_iter().collect()
    }

    pub fn sync_token(&self, folder: &FolderId) -> Result<SyncToken> {
        let token = self.conn.query_row("SELECT sync_token FROM folders WHERE id = ?1", [&folder.0], |r| r.get(0)).optional()?;
        Ok(SyncToken(token.unwrap_or_default()))
    }

    // ── messages ───────────────────────────────────────────────────────────────────────────

    /// Apply one sync result atomically and store its token.
    pub fn apply_delta(&mut self, account: &AccountId, folder: &FolderId, delta: &FolderDelta) -> Result<()> {
        let tx = self.conn.transaction()?;
        for e in &delta.added { rows::upsert_envelope(&tx, account, e)?; }
        for (id, flags) in &delta.changed_flags {
            tx.execute("UPDATE messages SET flags = ?2 WHERE id = ?1", params![id.0, flags.0])?;
        }
        for id in &delta.removed { rows::delete_message(&tx, id)?; }
        tx.execute("UPDATE folders SET sync_token = ?2 WHERE id = ?1", params![folder.0, delta.token.0])?;
        Ok(tx.commit()?)
    }

    /// A page of a folder, newest first.
    pub fn envelopes(&self, folder: &FolderId, offset: u32, limit: u32) -> Result<Vec<Envelope>> {
        let sql = format!("SELECT {} FROM messages m WHERE m.folder = ?1 ORDER BY m.date DESC, m.id LIMIT ?3 OFFSET ?2", rows::ENVELOPE_COLUMNS);
        let mut q = self.conn.prepare_cached(&sql)?;
        let found = q.query_map(params![folder.0, offset, limit], rows::envelope)?.collect::<rusqlite::Result<Vec<_>>>()?;
        found.into_iter().collect()
    }

    pub fn envelope(&self, id: &MessageId) -> Result<Option<Envelope>> {
        let sql = format!("SELECT {} FROM messages m WHERE m.id = ?1", rows::ENVELOPE_COLUMNS);
        self.conn.query_row(&sql, [&id.0], rows::envelope).optional()?.transpose()
    }

    pub fn account_of(&self, id: &MessageId) -> Result<Option<AccountId>> {
        Ok(self.conn.query_row("SELECT account FROM messages WHERE id = ?1", [&id.0], |r| r.get(0)).optional()?.map(AccountId))
    }

    pub fn set_flags(&mut self, ids: &[MessageId], add: Flags, remove: Flags) -> Result<()> {
        let tx = self.conn.transaction()?;
        for id in ids {
            tx.execute("UPDATE messages SET flags = (flags | ?2) & ~?3 WHERE id = ?1", params![id.0, add.0, remove.0])?;
        }
        Ok(tx.commit()?)
    }

    pub fn move_messages(&mut self, ids: &[MessageId], to: &FolderId) -> Result<()> {
        let tx = self.conn.transaction()?;
        for id in ids { tx.execute("UPDATE messages SET folder = ?2 WHERE id = ?1", params![id.0, to.0])?; }
        Ok(tx.commit()?)
    }

    // ── bodies ─────────────────────────────────────────────────────────────────────────────

    /// Cache a fetched message and index its text; evicts least-recently-opened bodies over the cap.
    pub fn put_body(&mut self, raw: &RawMessage, text: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let stamp: i64 = tx.query_row("SELECT coalesce(max(last_opened), 0) + 1 FROM bodies", [], |r| r.get(0))?;
        tx.execute(
            "INSERT OR REPLACE INTO bodies (message_id, raw, size, last_opened) VALUES (?1, ?2, ?3, ?4)",
            params![raw.id.0, raw.bytes, raw.bytes.len() as i64, stamp],
        )?;
        tx.execute("UPDATE search SET body = ?2 WHERE rowid = (SELECT rowid FROM messages WHERE id = ?1)", params![raw.id.0, text])?;
        let mut total: i64 = tx.query_row("SELECT coalesce(sum(size), 0) FROM bodies", [], |r| r.get(0))?;
        while total as u64 > self.body_bytes_cap {
            let oldest: Option<(String, i64)> = tx
                .query_row("SELECT message_id, size FROM bodies WHERE message_id != ?1 ORDER BY last_opened LIMIT 1", [&raw.id.0], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((id, size)) = oldest else { break };
            tx.execute("DELETE FROM bodies WHERE message_id = ?1", [&id])?;
            total -= size;
        }
        Ok(tx.commit()?)
    }

    /// A cached body, marked as just opened.
    pub fn body(&mut self, id: &MessageId) -> Result<Option<RawMessage>> {
        let bytes: Option<Vec<u8>> = self.conn.query_row("SELECT raw FROM bodies WHERE message_id = ?1", [&id.0], |r| r.get(0)).optional()?;
        if bytes.is_some() {
            self.conn.execute("UPDATE bodies SET last_opened = (SELECT max(last_opened) + 1 FROM bodies) WHERE message_id = ?1", [&id.0])?;
        }
        Ok(bytes.map(|bytes| RawMessage { id: id.clone(), bytes }))
    }

    // ── search ─────────────────────────────────────────────────────────────────────────────

    /// Local search, newest first. `account` limits it to one account.
    pub fn search(&self, query: &Query, account: Option<&AccountId>, limit: u32) -> Result<Vec<Envelope>> {
        let mut sql = format!("SELECT {} FROM messages m", rows::ENVELOPE_COLUMNS);
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        let mut filters: Vec<String> = Vec::new();
        let bind = |args: &mut Vec<rusqlite::types::Value>, v: rusqlite::types::Value| { args.push(v); format!("?{}", args.len()) };
        if let Some(m) = query.fts_match() {
            sql.push_str(" JOIN search s ON s.rowid = m.rowid");
            filters.push(format!("search MATCH {}", bind(&mut args, m.into())));
        }
        if let Some(a) = account { filters.push(format!("m.account = {}", bind(&mut args, a.0.clone().into()))); }
        if let Some(f) = &query.folder {
            let p = bind(&mut args, f.clone().into());
            filters.push(format!("m.folder IN (SELECT id FROM folders WHERE name = {p} COLLATE NOCASE OR id = {p})"));
        }
        if query.has_attachment { filters.push("m.attachments != '[]'".into()); }
        if let Some(unread) = query.unread { filters.push(format!("m.flags & 1 {} 0", if unread { "=" } else { "!=" })); }
        if let Some(flagged) = query.flagged { filters.push(format!("m.flags & 2 {} 0", if flagged { "!=" } else { "=" })); }
        if let Some(t) = query.before { filters.push(format!("m.date < {}", bind(&mut args, t.into()))); }
        if let Some(t) = query.after { filters.push(format!("m.date >= {}", bind(&mut args, t.into()))); }
        if !filters.is_empty() { sql.push_str(" WHERE "); sql.push_str(&filters.join(" AND ")); }
        sql.push_str(&format!(" ORDER BY m.date DESC, m.id LIMIT {limit}"));
        let mut q = self.conn.prepare(&sql)?;
        let found = q.query_map(rusqlite::params_from_iter(args), rows::envelope)?.collect::<rusqlite::Result<Vec<_>>>()?;
        found.into_iter().collect()
    }

    // ── journal, contacts, image policy ────────────────────────────────────────────────────

    /// Append a pending offline operation (JSON owned by the engine). Returns its sequence number.
    pub fn journal_push(&mut self, op: &str) -> Result<i64> {
        self.conn.execute("INSERT INTO ops_journal (op) VALUES (?1)", [op])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn journal_pending(&self) -> Result<Vec<(i64, String)>> {
        let mut q = self.conn.prepare_cached("SELECT seq, op FROM ops_journal ORDER BY seq")?;
        Ok(q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn journal_remove(&mut self, seq: i64) -> Result<()> {
        self.conn.execute("DELETE FROM ops_journal WHERE seq = ?1", [seq])?;
        Ok(())
    }

    /// Address completion: seen addresses matching a prefix of the name or email, most used first.
    pub fn complete_address(&self, prefix: &str, limit: u32) -> Result<Vec<Address>> {
        let like = format!("{}%", prefix.replace(['%', '_'], ""));
        let mut q = self.conn.prepare_cached("SELECT name, email FROM contacts WHERE email LIKE ?1 OR name LIKE ?1 ORDER BY uses DESC, email LIMIT ?2")?;
        Ok(q.query_map(params![like, limit], |r| Ok(Address { name: r.get(0)?, email: r.get(1)? }))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn allow_images_from(&mut self, sender: &str) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO image_policy (sender) VALUES (?1)", [sender.to_ascii_lowercase()])?;
        Ok(())
    }

    pub fn images_allowed_from(&self, sender: &str) -> Result<bool> {
        Ok(self.conn.query_row("SELECT 1 FROM image_policy WHERE sender = ?1", [sender.to_ascii_lowercase()], |_| Ok(())).optional()?.is_some())
    }
}

//! Durable state for users mode, in one SQLite file: users, their mail accounts and the secrets
//! those accounts sign in with. Sessions are not here; they live in memory only.
//!
//! What is in clear: user ids, usernames (for OIDC users, the email address the identity
//! provider vouched for) and Argon2id hashes. What is sealed with the user's key
//! ([`crate::crypto`]): every account's settings (which hold its address and server names) and
//! every password or refresh token. Account ids are stored blinded, so the file does not say
//! which accounts anyone has either.

use crate::crypto::{MasterKey, UserKey};
use mail_auth::secrets::{SecretError, SecretKind, SecretStore};
use mail_auth::AccountConfig;
use mail_core::AccountId;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The name or id is taken.
    Conflict,
    NotFound,
    /// A sealed value does not open: the master key changed or the row was tampered with.
    Sealed,
    /// The database failed. The text is SQLite's, never a value.
    Db(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Conflict => write!(f, "already exists"),
            StoreError::NotFound => write!(f, "not found"),
            StoreError::Sealed => write!(f, "a stored secret does not decrypt with this master key"),
            StoreError::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::ConstraintViolation) => StoreError::Conflict,
            _ => StoreError::Db(e.sqlite_error_code().map_or_else(|| "query failed".to_owned(), |c| format!("{c:?}"))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User { pub id: i64, pub name: String }

pub struct Store { db: Mutex<Connection>, master: MasterKey }

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    password TEXT,
    oidc TEXT UNIQUE
);
CREATE TABLE IF NOT EXISTS accounts (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    sealed BLOB NOT NULL,
    PRIMARY KEY (user_id, key)
);
CREATE TABLE IF NOT EXISTS secrets (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    account TEXT NOT NULL,
    kind TEXT NOT NULL,
    sealed BLOB NOT NULL,
    PRIMARY KEY (user_id, account, kind)
);
";

fn kind_label(kind: SecretKind) -> &'static str { match kind { SecretKind::Password => "password", SecretKind::RefreshToken => "refresh-token" } }

impl Store {
    /// Open (or create) the database file. On Unix a new file is made private to its owner.
    pub fn open(path: &Path, master: MasterKey) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Db(e.kind().to_string()))?;
        }
        let fresh = !path.exists();
        let db = Connection::open(path)?;
        #[cfg(unix)]
        if fresh {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        #[cfg(not(unix))]
        let _ = fresh;
        Self::init(db, master)
    }

    pub fn open_in_memory(master: MasterKey) -> Result<Self, StoreError> { Self::init(Connection::open_in_memory()?, master) }

    fn init(db: Connection, master: MasterKey) -> Result<Self, StoreError> {
        db.execute_batch(SCHEMA)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self { db: Mutex::new(db), master })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let db = self.db.lock().map_err(|_| StoreError::Db("poisoned".into()))?;
        f(&db)
    }

    fn key(&self, user: i64) -> UserKey { self.master.user_key(user) }

    // Users.

    pub fn add_user(&self, name: &str, password_hash: Option<&str>) -> Result<User, StoreError> {
        self.with(|db| {
            db.execute("INSERT INTO users (name, password) VALUES (?1, ?2)", params![name, password_hash])?;
            Ok(User { id: db.last_insert_rowid(), name: name.to_owned() })
        })
    }

    pub fn set_password(&self, name: &str, password_hash: &str) -> Result<(), StoreError> {
        self.with(|db| match db.execute("UPDATE users SET password = ?2 WHERE name = ?1", params![name, password_hash])? { 0 => Err(StoreError::NotFound), _ => Ok(()) })
    }

    /// Remove a user and, by cascade, every account and secret they had.
    pub fn remove_user(&self, name: &str) -> Result<Option<User>, StoreError> {
        self.with(|db| {
            let user = db.query_row("SELECT id, name FROM users WHERE name = ?1", [name], |r| Ok(User { id: r.get(0)?, name: r.get(1)? })).optional()?;
            if let Some(user) = &user { db.execute("DELETE FROM users WHERE id = ?1", [user.id])?; }
            Ok(user)
        })
    }

    pub fn users(&self) -> Result<Vec<User>, StoreError> {
        self.with(|db| {
            let mut statement = db.prepare("SELECT id, name FROM users ORDER BY name")?;
            let rows = statement.query_map([], |r| Ok(User { id: r.get(0)?, name: r.get(1)? }))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
    }

    pub fn user(&self, id: i64) -> Result<Option<User>, StoreError> {
        self.with(|db| Ok(db.query_row("SELECT id, name FROM users WHERE id = ?1", [id], |r| Ok(User { id: r.get(0)?, name: r.get(1)? })).optional()?))
    }

    /// The user and their password hash, for a password sign-in.
    pub fn password_of(&self, name: &str) -> Result<Option<(User, String)>, StoreError> {
        self.with(|db| {
            let row = db.query_row("SELECT id, name, password FROM users WHERE name = ?1", [name], |r| Ok((User { id: r.get(0)?, name: r.get(1)? }, r.get::<_, Option<String>>(2)?))).optional()?;
            Ok(row.and_then(|(user, hash)| Some((user, hash?))))
        })
    }

    /// The user an identity provider's subject belongs to, created on first sign-in. `subject`
    /// is `issuer|sub`; it is stored hashed. A name already taken by someone else gets a suffix.
    pub fn oidc_user(&self, subject: &str, name: &str) -> Result<User, StoreError> {
        let subject = crate::crypto::digest(subject);
        self.with(|db| {
            if let Some(user) = db.query_row("SELECT id, name FROM users WHERE oidc = ?1", [&subject], |r| Ok(User { id: r.get(0)?, name: r.get(1)? })).optional()? { return Ok(user); }
            for attempt in 0..100 {
                let candidate = if attempt == 0 { name.to_owned() } else { format!("{name}#{attempt}") };
                match db.execute("INSERT INTO users (name, oidc) VALUES (?1, ?2)", params![candidate, subject]) {
                    Ok(_) => return Ok(User { id: db.last_insert_rowid(), name: candidate }),
                    Err(e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            Err(StoreError::Conflict)
        })
    }

    // Accounts.

    fn account_context(user: i64, key: &str) -> String { format!("{user}/{key}/config") }

    pub fn add_account(&self, user: i64, config: &AccountConfig) -> Result<(), StoreError> {
        let user_key = self.key(user);
        let key = user_key.blind(&config.id.0);
        let json = serde_json::to_vec(config).map_err(|e| StoreError::Db(e.to_string()))?;
        let sealed = user_key.seal(&Self::account_context(user, &key), &json);
        self.with(|db| {
            db.execute("INSERT INTO accounts (user_id, key, sealed) VALUES (?1, ?2, ?3)", params![user, key, sealed])?;
            Ok(())
        })
    }

    pub fn account(&self, user: i64, id: &AccountId) -> Result<Option<AccountConfig>, StoreError> {
        let user_key = self.key(user);
        let key = user_key.blind(&id.0);
        let sealed: Option<Vec<u8>> = self.with(|db| Ok(db.query_row("SELECT sealed FROM accounts WHERE user_id = ?1 AND key = ?2", params![user, key], |r| r.get(0)).optional()?))?;
        sealed.map(|sealed| open_config(&user_key, &Self::account_context(user, &key), &sealed)).transpose()
    }

    pub fn accounts(&self, user: i64) -> Result<Vec<AccountConfig>, StoreError> {
        let user_key = self.key(user);
        let rows: Vec<(String, Vec<u8>)> = self.with(|db| {
            let mut statement = db.prepare("SELECT key, sealed FROM accounts WHERE user_id = ?1")?;
            let rows = statement.query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })?;
        let mut configs: Vec<AccountConfig> = rows.iter().map(|(key, sealed)| open_config(&user_key, &Self::account_context(user, key), sealed)).collect::<Result<_, _>>()?;
        configs.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(configs)
    }

    /// Remove an account and its secrets. `false` if the user has no such account.
    pub fn remove_account(&self, user: i64, id: &AccountId) -> Result<bool, StoreError> {
        let key = self.key(user).blind(&id.0);
        self.with(|db| {
            db.execute("DELETE FROM secrets WHERE user_id = ?1 AND account = ?2", params![user, key])?;
            Ok(db.execute("DELETE FROM accounts WHERE user_id = ?1 AND key = ?2", params![user, key])? > 0)
        })
    }

    // Secrets.

    fn secret_context(user: i64, account: &str, kind: SecretKind) -> String { format!("{user}/{account}/{}", kind_label(kind)) }

    pub fn secret(&self, user: i64, account: &AccountId, kind: SecretKind) -> Result<Option<String>, StoreError> {
        let user_key = self.key(user);
        let key = user_key.blind(&account.0);
        let sealed: Option<Vec<u8>> = self.with(|db| Ok(db.query_row("SELECT sealed FROM secrets WHERE user_id = ?1 AND account = ?2 AND kind = ?3", params![user, key, kind_label(kind)], |r| r.get(0)).optional()?))?;
        let Some(sealed) = sealed else { return Ok(None) };
        let plain = user_key.open(&Self::secret_context(user, &key, kind), &sealed).map_err(|_| StoreError::Sealed)?;
        String::from_utf8(plain).map(Some).map_err(|_| StoreError::Sealed)
    }

    pub fn set_secret(&self, user: i64, account: &AccountId, kind: SecretKind, secret: &str) -> Result<(), StoreError> {
        let user_key = self.key(user);
        let key = user_key.blind(&account.0);
        let sealed = user_key.seal(&Self::secret_context(user, &key, kind), secret.as_bytes());
        self.with(|db| {
            db.execute("INSERT OR REPLACE INTO secrets (user_id, account, kind, sealed) VALUES (?1, ?2, ?3, ?4)", params![user, key, kind_label(kind), sealed])?;
            Ok(())
        })
    }

    pub fn delete_secret(&self, user: i64, account: &AccountId, kind: SecretKind) -> Result<(), StoreError> {
        let key = self.key(user).blind(&account.0);
        self.with(|db| {
            db.execute("DELETE FROM secrets WHERE user_id = ?1 AND account = ?2 AND kind = ?3", params![user, key, kind_label(kind)])?;
            Ok(())
        })
    }
}

fn open_config(key: &UserKey, context: &str, sealed: &[u8]) -> Result<AccountConfig, StoreError> {
    let json = key.open(context, sealed).map_err(|_| StoreError::Sealed)?;
    serde_json::from_slice(&json).map_err(|_| StoreError::Sealed)
}

/// One user's secrets as a [`SecretStore`], so `ravenglass_accounts::Connector` uses them
/// unchanged. It cannot name another user: the user is fixed when it is made.
pub struct UserSecrets { store: Arc<Store>, user: i64 }

impl UserSecrets {
    pub fn new(store: Arc<Store>, user: i64) -> Self { Self { store, user } }
}

fn secret_error(e: StoreError) -> SecretError { SecretError(e.to_string()) }

impl SecretStore for UserSecrets {
    fn get(&self, account: &AccountId, kind: SecretKind) -> Result<Option<String>, SecretError> { self.store.secret(self.user, account, kind).map_err(secret_error) }
    fn set(&self, account: &AccountId, kind: SecretKind, secret: &str) -> Result<(), SecretError> { self.store.set_secret(self.user, account, kind, secret).map_err(secret_error) }
    fn delete(&self, account: &AccountId, kind: SecretKind) -> Result<(), SecretError> { self.store.delete_secret(self.user, account, kind).map_err(secret_error) }
}

#[cfg(test)]
pub(crate) mod tests;

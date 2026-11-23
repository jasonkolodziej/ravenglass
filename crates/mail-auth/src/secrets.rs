//! Secret storage (architecture §12): the OS keyring on desktop, memory in tests. The web build
//! keeps secrets on the Relay instead and uses neither.

use mail_core::AccountId;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecretKind { Password, RefreshToken }

impl SecretKind {
    #[cfg(feature = "keyring")]
    fn label(self) -> &'static str { match self { SecretKind::Password => "password", SecretKind::RefreshToken => "refresh-token" } }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the credential store is unavailable: {0}")]
pub struct SecretError(pub String);

pub trait SecretStore: Send + Sync {
    fn get(&self, account: &AccountId, kind: SecretKind) -> Result<Option<String>, SecretError>;
    fn set(&self, account: &AccountId, kind: SecretKind, secret: &str) -> Result<(), SecretError>;
    fn delete(&self, account: &AccountId, kind: SecretKind) -> Result<(), SecretError>;

    /// Forget everything about an account (used when it is removed).
    fn delete_all(&self, account: &AccountId) -> Result<(), SecretError> {
        self.delete(account, SecretKind::Password)?;
        self.delete(account, SecretKind::RefreshToken)
    }
}

/// Secrets held in memory only. For tests and for "this session only" passwords.
#[derive(Default)]
pub struct MemorySecrets(Mutex<BTreeMap<(AccountId, SecretKind), String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, account: &AccountId, kind: SecretKind) -> Result<Option<String>, SecretError> {
        Ok(self.0.lock().map_err(|e| SecretError(e.to_string()))?.get(&(account.clone(), kind)).cloned())
    }
    fn set(&self, account: &AccountId, kind: SecretKind, secret: &str) -> Result<(), SecretError> {
        self.0.lock().map_err(|e| SecretError(e.to_string()))?.insert((account.clone(), kind), secret.to_owned());
        Ok(())
    }
    fn delete(&self, account: &AccountId, kind: SecretKind) -> Result<(), SecretError> {
        self.0.lock().map_err(|e| SecretError(e.to_string()))?.remove(&(account.clone(), kind));
        Ok(())
    }
}

/// Keychain (macOS), Credential Manager (Windows) or Secret Service (Linux).
#[cfg(feature = "keyring")]
pub struct KeyringSecrets { service: String }

#[cfg(feature = "keyring")]
impl KeyringSecrets {
    pub fn new() -> Self { Self { service: "Ravenglass".into() } }

    fn entry(&self, account: &AccountId, kind: SecretKind) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(&self.service, &format!("{}/{}", account.0, kind.label())).map_err(|e| SecretError(e.to_string()))
    }
}

#[cfg(feature = "keyring")]
impl Default for KeyringSecrets {
    fn default() -> Self { Self::new() }
}

#[cfg(feature = "keyring")]
impl SecretStore for KeyringSecrets {
    fn get(&self, account: &AccountId, kind: SecretKind) -> Result<Option<String>, SecretError> {
        match self.entry(account, kind)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError(e.to_string())),
        }
    }
    fn set(&self, account: &AccountId, kind: SecretKind, secret: &str) -> Result<(), SecretError> {
        self.entry(account, kind)?.set_password(secret).map_err(|e| SecretError(e.to_string()))
    }
    fn delete(&self, account: &AccountId, kind: SecretKind) -> Result<(), SecretError> {
        match self.entry(account, kind)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError(e.to_string())),
        }
    }
}

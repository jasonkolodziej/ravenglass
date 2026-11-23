//! The app-lock passcode: the way to open the lock when the OS has no prompt of its own (or it is
//! not wired up yet) and no account has a password to check against.
//!
//! Only an Argon2id hash is stored (a PHC string: algorithm, parameters, a random salt and the
//! hash), in the same secret store as account passwords, under an id no account can have. The
//! passcode itself is never stored, logged or compared as text.

use crate::secrets::{SecretError, SecretKind, SecretStore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use mail_core::AccountId;

/// The shortest passcode accepted, in characters.
pub const MIN_LEN: usize = 6;

/// Where the hash lives in the secret store. Account ids are letters, digits, `-`, `_` and `.`,
/// so the `:` keeps this from ever naming an account.
pub fn store_id() -> AccountId { AccountId::from("ravenglass:app-lock") }

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PasscodeError {
    #[error("a passcode needs at least {MIN_LEN} characters")]
    TooShort,
    #[error("no secure random numbers: {0}")]
    Random(String),
    #[error("could not hash the passcode")]
    Hash,
    #[error(transparent)]
    Store(#[from] SecretError),
}

/// The Argon2id hash of `passcode` with a fresh salt, as a PHC string.
pub fn hash(passcode: &str) -> Result<String, PasscodeError> {
    if passcode.chars().count() < MIN_LEN { return Err(PasscodeError::TooShort); }
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| PasscodeError::Random(e.to_string()))?;
    let salt = SaltString::encode_b64(&salt).map_err(|_| PasscodeError::Hash)?;
    Argon2::default().hash_password(passcode.as_bytes(), &salt).map(|h| h.to_string()).map_err(|_| PasscodeError::Hash)
}

/// Whether `passcode` matches the stored `phc` hash. A malformed hash matches nothing.
pub fn verify(passcode: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else { return false };
    Argon2::default().verify_password(passcode.as_bytes(), &parsed).is_ok()
}

pub fn is_set(store: &dyn SecretStore) -> Result<bool, SecretError> {
    Ok(store.get(&store_id(), SecretKind::Password)?.is_some())
}

/// Hash and store a new passcode, replacing any old one.
pub fn set(store: &dyn SecretStore, passcode: &str) -> Result<(), PasscodeError> {
    let phc = hash(passcode)?;
    Ok(store.set(&store_id(), SecretKind::Password, &phc)?)
}

/// Whether `typed` is the stored passcode. False when none is stored or the store fails.
pub fn check(store: &dyn SecretStore, typed: &str) -> bool {
    matches!(store.get(&store_id(), SecretKind::Password), Ok(Some(phc)) if verify(typed, &phc))
}

pub fn clear(store: &dyn SecretStore) -> Result<(), SecretError> { store.delete(&store_id(), SecretKind::Password) }

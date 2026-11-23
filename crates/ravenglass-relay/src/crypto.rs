//! Keys and sealing (architecture §12, "Relay storage").
//!
//! One 32-byte master key lives in a file of its own (mode 0600, created on first start). Each
//! user's key is derived from it with HKDF-SHA256, so a leaked user key exposes one user only and
//! the master key never touches data directly. Sealed values are AES-256-GCM with a fresh random
//! 96-bit nonce and associated data naming the row they belong to, so a ciphertext copied into
//! another user's or another account's row fails to open.
//!
//! No key, plaintext or nonce is ever printed: the key types have no `Debug`, and errors name
//! the problem, never the content.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use std::path::Path;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const HKDF_SALT: &[u8] = b"ravenglass-relay/v1/user-key";

#[derive(Debug, PartialEq, Eq)]
pub enum KeyError {
    /// The file exists but is not exactly 32 bytes.
    Malformed,
    Io(String),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::Malformed => write!(f, "the master key file must hold exactly 32 bytes"),
            KeyError::Io(e) => write!(f, "cannot read or create the master key file: {e}"),
        }
    }
}

impl std::error::Error for KeyError {}

/// Fill `buf` from the operating system's CSPRNG.
pub fn random(buf: &mut [u8]) {
    getrandom::fill(buf).expect("the operating system has no random number generator");
}

/// `n` random bytes, base64url without padding.
pub fn random_token(n: usize) -> String {
    use base64::Engine;
    let mut bytes = vec![0u8; n];
    random(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// SHA-256 of a string, hex. Sessions are stored by this, never by the token itself.
pub fn digest(text: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Equal, compared in constant time for equal lengths.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Argon2id cost. `Standard` is OWASP's minimum (19 MiB, 2 passes); `Weak` is for tests only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordCost { Standard, Weak }

fn argon2(cost: PasswordCost) -> argon2::Argon2<'static> {
    let params = match cost {
        PasswordCost::Standard => argon2::Params::new(19 * 1024, 2, 1, None),
        PasswordCost::Weak => argon2::Params::new(8, 1, 1, None),
    }
    .expect("valid Argon2 parameters");
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
}

/// An Argon2id PHC string for `password`, with a fresh salt.
pub fn hash_password(password: &str, cost: PasswordCost) -> String {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let mut salt = [0u8; 16];
    random(&mut salt);
    let salt = SaltString::encode_b64(&salt).expect("16 bytes is a valid salt");
    argon2(cost).hash_password(password.as_bytes(), &salt).expect("Argon2 hashing does not fail for valid parameters").to_string()
}

/// Whether `password` matches the PHC string. The parameters come from the string itself; the
/// final comparison is constant-time.
pub fn verify_password(password: &str, phc: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(phc).is_ok_and(|hash| argon2::Argon2::default().verify_password(password.as_bytes(), &hash).is_ok())
}

pub struct MasterKey([u8; KEY_LEN]);

impl MasterKey {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self { Self(bytes) }

    /// A fresh random key, for tests and first start.
    pub fn generate() -> Self {
        let mut key = [0u8; KEY_LEN];
        random(&mut key);
        Self(key)
    }

    /// Read the key at `path`, or create it there (mode 0600, parent directories too) if absent.
    /// Returns whether it was created.
    pub fn load_or_create(path: &Path) -> Result<(Self, bool), KeyError> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let key: [u8; KEY_LEN] = bytes.try_into().map_err(|_| KeyError::Malformed)?;
                Ok((Self(key), false))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent).map_err(|e| KeyError::Io(e.kind().to_string()))?;
                }
                let key = Self::generate();
                write_private(path, &key.0).map_err(|e| KeyError::Io(e.kind().to_string()))?;
                Ok((key, true))
            }
            Err(e) => Err(KeyError::Io(e.kind().to_string())),
        }
    }

    /// The keys for one user's data: one seals values, the other blinds lookup keys.
    pub fn user_key(&self, user: i64) -> UserKey {
        UserKey { seal: self.derive(&format!("user:{user}:seal")), index: self.derive(&format!("user:{user}:index")) }
    }

    fn derive(&self, info: &str) -> [u8; KEY_LEN] {
        let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(HKDF_SALT), &self.0);
        let mut key = [0u8; KEY_LEN];
        hkdf.expand(info.as_bytes(), &mut key).expect("32 bytes is a valid HKDF-SHA256 output length");
        key
    }
}

/// Whether a key file can be read by anyone but its owner (Unix only; elsewhere `false`).
pub fn readable_by_others(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o077 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

pub struct UserKey { seal: [u8; KEY_LEN], index: [u8; KEY_LEN] }

#[derive(Debug, PartialEq, Eq)]
pub struct SealError;

impl UserKey {
    /// A stable, opaque lookup key for `name` (an account id), so the database holds no ids in clear.
    pub fn blind(&self, name: &str) -> String {
        use sha2::Digest;
        let mut hash = sha2::Sha256::new();
        hash.update(self.index);
        hash.update(name.as_bytes());
        hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// `nonce || ciphertext+tag`, bound to `context`.
    pub fn seal(&self, context: &str, plaintext: &[u8]) -> Vec<u8> {
        let cipher = Aes256Gcm::new_from_slice(&self.seal).expect("the key is 32 bytes");
        let mut nonce = [0u8; NONCE_LEN];
        random(&mut nonce);
        let sealed = cipher.encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: context.as_bytes() }).expect("AES-GCM encryption does not fail for in-memory buffers");
        let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        out
    }

    /// The plaintext, if `sealed` was made by this key for this `context` and is unchanged.
    pub fn open(&self, context: &str, sealed: &[u8]) -> Result<Vec<u8>, SealError> {
        if sealed.len() < NONCE_LEN { return Err(SealError); }
        let cipher = Aes256Gcm::new_from_slice(&self.seal).map_err(|_| SealError)?;
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        cipher.decrypt(Nonce::from_slice(nonce), Payload { msg: body, aad: context.as_bytes() }).map_err(|_| SealError)
    }
}

#[cfg(test)]
mod tests;

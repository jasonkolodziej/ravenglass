//! Verifying a signed JWT (a JWS in compact serialization, RFC 7515) against the identity
//! provider's published keys (a JWK Set, RFC 7517), for OIDC ID tokens.
//!
//! Signature verification only, in pure Rust (RustCrypto): RSA PKCS#1 v1.5 and PSS, ECDSA on
//! P-256 and P-384, and Ed25519. Nothing here decrypts or signs, and nothing here does I/O.
//!
//! The rules:
//!
//! - The `alg` must be on [`Alg`]'s allowlist: never `none`, never an HMAC algorithm.
//! - The key comes only from the key set the caller fetched from the provider. Header fields that
//!   name or carry a key (`jku`, `x5u`, `jwk`, `x5c`) are ignored, and a `crit` header is refused.
//! - The key is picked by `kid`; with no `kid`, it must be the only key that fits the `alg`.
//! - The `alg` must fit the key's type and curve, and the key's own `alg` when it states one.
//! - RSA keys under 2048 bits are not used.
//!
//! Only after the signature verifies does [`verify`] return the payload.

use base64::Engine;
use serde::Deserialize;

/// Smallest RSA modulus accepted, in bits.
pub const MIN_RSA_BITS: usize = 2048;
/// Largest RSA modulus accepted, in bits (bounds the work one verification costs).
pub const MAX_RSA_BITS: usize = 8192;

/// The signature algorithms accepted (RFC 7518 §3, RFC 8037 §3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alg { Rs256, Rs384, Rs512, Ps256, Es256, Es384, EdDsa }

impl Alg {
    /// The allowlist. Anything else (`none`, `HS256`, `PS512`, …) is `None`.
    pub fn parse(name: &str) -> Option<Alg> {
        Some(match name {
            "RS256" => Alg::Rs256,
            "RS384" => Alg::Rs384,
            "RS512" => Alg::Rs512,
            "PS256" => Alg::Ps256,
            "ES256" => Alg::Es256,
            "ES384" => Alg::Es384,
            "EdDSA" => Alg::EdDsa,
            _ => return None,
        })
    }

    fn fits(self, material: &Material) -> bool {
        matches!(
            (self, material),
            (Alg::Rs256 | Alg::Rs384 | Alg::Rs512 | Alg::Ps256, Material::Rsa(_))
                | (Alg::Es256, Material::P256(_))
                | (Alg::Es384, Material::P384(_))
                | (Alg::EdDsa, Material::Ed25519(_))
        )
    }
}

/// Why a token was refused. Callers show one generic error for all of them; the variants exist
/// so the caller can tell [`JoseError::UnknownKey`] (worth refetching the key set) from the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoseError {
    /// Not three base64url parts, or a header or payload that is not JSON.
    Malformed,
    /// An `alg` off the allowlist, or a `crit` header.
    Unsupported,
    /// No key in the set has the token's `kid` (or, with no `kid`, fits its `alg`).
    UnknownKey,
    /// The key named by `kid` is of another type, curve or `alg` than the token's `alg`.
    KeyMismatch,
    /// With no `kid`, more than one key fits.
    Ambiguous,
    BadSignature,
}

impl JoseError {
    /// A short reason, safe to log: it never contains the token or any part of it.
    pub fn reason(self) -> &'static str {
        match self {
            JoseError::Malformed => "malformed",
            JoseError::Unsupported => "algorithm not allowed",
            JoseError::UnknownKey => "unknown key",
            JoseError::KeyMismatch => "algorithm does not fit the key",
            JoseError::Ambiguous => "no kid and several keys fit",
            JoseError::BadSignature => "bad signature",
        }
    }
}

enum Material {
    Rsa(rsa::PublicKey),
    P256(p256::ecdsa::VerifyingKey),
    P384(p384::ecdsa::VerifyingKey),
    Ed25519(ed25519_dalek::VerifyingKey),
}

/// One usable verification key from the provider's key set.
pub struct Key { kid: Option<String>, alg: Option<Alg>, material: Material }

/// The provider's signing keys, as published at its `jwks_uri`.
#[derive(Default)]
pub struct KeySet { keys: Vec<Key> }

impl KeySet {
    pub fn len(&self) -> usize { self.keys.len() }
    pub fn is_empty(&self) -> bool { self.keys.is_empty() }
}

/// A JWK as published: every field optional, so one odd key does not spoil the set.
#[derive(Deserialize)]
struct Jwk {
    kty: Option<String>,
    kid: Option<String>,
    alg: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
    key_ops: Option<Vec<String>>,
    crv: Option<String>,
    n: Option<String>,
    e: Option<String>,
    x: Option<String>,
    y: Option<String>,
}

#[derive(Deserialize)]
struct JwkSet { keys: Vec<serde_json::Value> }

fn b64(text: &str) -> Option<Vec<u8>> { base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text).ok() }

/// Parse a JWK Set. Keys that are not for verifying signatures with an allowed algorithm
/// (encryption keys, symmetric keys, unknown curves, short RSA keys, broken values) are left out.
/// `None` when the document is not a JWK Set at all.
pub fn parse_key_set(json: &[u8]) -> Option<KeySet> {
    let set: JwkSet = serde_json::from_slice(json).ok()?;
    let keys = set.keys.into_iter().filter_map(|value| serde_json::from_value::<Jwk>(value).ok()).filter_map(key).collect();
    Some(KeySet { keys })
}

fn key(jwk: Jwk) -> Option<Key> {
    if jwk.usage.as_deref().is_some_and(|u| u != "sig") { return None; }
    if jwk.key_ops.as_ref().is_some_and(|ops| !ops.iter().any(|op| op == "verify")) { return None; }
    let alg = match jwk.alg.as_deref() { Some(name) => Some(Alg::parse(name)?), None => None };
    let material = match (jwk.kty.as_deref()?, jwk.crv.as_deref()) {
        ("RSA", _) => {
            let (n, e) = (b64(jwk.n.as_deref()?)?, b64(jwk.e.as_deref()?)?);
            let n = rsa::BoxedUint::from_be_slice_vartime(&n);
            let e = rsa::BoxedUint::from_be_slice_vartime(&e);
            Material::Rsa(rsa::PublicKey::new_with_size_limits(n, e, MIN_RSA_BITS..=MAX_RSA_BITS).ok()?)
        }
        ("EC", Some("P-256")) => Material::P256(p256::ecdsa::VerifyingKey::from_sec1_bytes(&sec1_point(&jwk, 32)?).ok()?),
        ("EC", Some("P-384")) => Material::P384(p384::ecdsa::VerifyingKey::from_sec1_bytes(&sec1_point(&jwk, 48)?).ok()?),
        ("OKP", Some("Ed25519")) => {
            let x: [u8; 32] = b64(jwk.x.as_deref()?)?.try_into().ok()?;
            Material::Ed25519(ed25519_dalek::VerifyingKey::from_bytes(&x).ok()?)
        }
        _ => return None,
    };
    if alg.is_some_and(|alg| !alg.fits(&material)) { return None; }
    Some(Key { kid: jwk.kid, alg, material })
}

/// `x` and `y` of an EC key as an uncompressed SEC1 point (RFC 7518 §6.2.1: each exactly `size` bytes).
fn sec1_point(jwk: &Jwk, size: usize) -> Option<Vec<u8>> {
    let (x, y) = (b64(jwk.x.as_deref()?)?, b64(jwk.y.as_deref()?)?);
    if x.len() != size || y.len() != size { return None; }
    let mut point = Vec::with_capacity(1 + 2 * size);
    point.push(0x04);
    point.extend_from_slice(&x);
    point.extend_from_slice(&y);
    Some(point)
}

/// The protected header. Only these fields are read; `jku`, `x5u`, `jwk`, `x5c` and the rest
/// are ignored, so a token cannot bring its own key.
#[derive(Deserialize)]
struct Header { alg: String, kid: Option<String>, crit: Option<serde_json::Value> }

/// Check `token`'s signature against `keys` and return its payload (the claims JSON).
pub fn verify(token: &str, keys: &KeySet) -> Result<Vec<u8>, JoseError> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else { return Err(JoseError::Malformed) };
    let header: Header = serde_json::from_slice(&b64(header_b64).ok_or(JoseError::Malformed)?).map_err(|_| JoseError::Malformed)?;
    if header.crit.is_some() { return Err(JoseError::Unsupported); }
    let alg = Alg::parse(&header.alg).ok_or(JoseError::Unsupported)?;
    let payload = b64(payload_b64).ok_or(JoseError::Malformed)?;
    let signature = b64(signature_b64).filter(|s| !s.is_empty()).ok_or(JoseError::Malformed)?;
    let candidates = candidates(keys, alg, header.kid.as_deref())?;
    // The signing input is the header and payload exactly as sent (RFC 7515 §5.2).
    let input = &token.as_bytes()[..header_b64.len() + 1 + payload_b64.len()];
    if candidates.iter().any(|key| check(alg, &key.material, input, &signature)) { Ok(payload) } else { Err(JoseError::BadSignature) }
}

fn candidates<'a>(keys: &'a KeySet, alg: Alg, kid: Option<&str>) -> Result<Vec<&'a Key>, JoseError> {
    let fits = |key: &&Key| alg.fits(&key.material) && key.alg.is_none_or(|a| a == alg);
    match kid {
        Some(kid) => {
            let named: Vec<&Key> = keys.keys.iter().filter(|k| k.kid.as_deref() == Some(kid)).collect();
            if named.is_empty() { return Err(JoseError::UnknownKey); }
            let fitting: Vec<&Key> = named.into_iter().filter(fits).collect();
            if fitting.is_empty() { Err(JoseError::KeyMismatch) } else { Ok(fitting) }
        }
        None => {
            let fitting: Vec<&Key> = keys.keys.iter().filter(fits).collect();
            match fitting.len() { 0 => Err(JoseError::UnknownKey), 1 => Ok(fitting), _ => Err(JoseError::Ambiguous) }
        }
    }
}

fn check(alg: Alg, material: &Material, input: &[u8], signature: &[u8]) -> bool {
    use rsa::signature::Verifier;
    use rsa::sha2::{Sha256, Sha384, Sha512};
    match (alg, material) {
        (Alg::Rs256 | Alg::Rs384 | Alg::Rs512 | Alg::Ps256, Material::Rsa(key)) => {
            if signature.len() != rsa::traits::PublicKeyParts::size(key) { return false; }
            let pkcs1 = || rsa::pkcs1v15::Signature::try_from(signature).ok();
            match alg {
                Alg::Rs256 => pkcs1().is_some_and(|s| rsa::pkcs1v15::VerifyingKey::<Sha256>::new(key.clone()).verify(input, &s).is_ok()),
                Alg::Rs384 => pkcs1().is_some_and(|s| rsa::pkcs1v15::VerifyingKey::<Sha384>::new(key.clone()).verify(input, &s).is_ok()),
                Alg::Rs512 => pkcs1().is_some_and(|s| rsa::pkcs1v15::VerifyingKey::<Sha512>::new(key.clone()).verify(input, &s).is_ok()),
                _ => {
                    // RFC 7518 §3.5: MGF1 with the same hash, and a salt as long as the hash.
                    let Ok(signature) = rsa::pss::Signature::try_from(signature) else { return false };
                    rsa::pss::VerifyingKey::<Sha256>::new(key.clone()).verify(input, &signature).is_ok()
                }
            }
        }
        (Alg::Es256, Material::P256(key)) => {
            let Ok(signature) = p256::ecdsa::Signature::from_slice(signature) else { return false };
            key.verify(input, &signature).is_ok()
        }
        (Alg::Es384, Material::P384(key)) => {
            let Ok(signature) = p384::ecdsa::Signature::from_slice(signature) else { return false };
            key.verify(input, &signature).is_ok()
        }
        (Alg::EdDsa, Material::Ed25519(key)) => {
            let Ok(bytes) = <[u8; 64]>::try_from(signature) else { return false };
            key.verify_strict(input, &ed25519_dalek::Signature::from_bytes(&bytes)).is_ok()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;

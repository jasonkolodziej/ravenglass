//! The fake identity provider's signing keys: generated per test run, published as a JWK Set,
//! and used to sign (and, for the attack tests, forge) ID tokens.

use base64::Engine;
use rsa::signature::{SignatureEncoding, Signer};
use serde_json::{Value, json};
use std::sync::OnceLock;

pub fn b64(bytes: &[u8]) -> String { base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes) }

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).unwrap();
    bytes
}

pub enum Key { Rsa(rsa::PrivateKey), P256(p256::ecdsa::SigningKey), Ed(ed25519_dalek::SigningKey) }

/// The provider's keys, and one an attacker holds.
pub struct Keys { pub rsa: Key, pub ec: Key, pub ed: Key, pub rotated: Key, pub attacker: Key }

/// One set per test binary: RSA key generation is the slow part.
pub fn keys() -> &'static Keys {
    static KEYS: OnceLock<Keys> = OnceLock::new();
    KEYS.get_or_init(|| Keys {
        rsa: Key::Rsa(rsa::PrivateKey::new(&mut getrandom::SysRng, 2048).unwrap()),
        ec: Key::P256(p256::ecdsa::SigningKey::from_slice(&random::<32>()).unwrap()),
        ed: Key::Ed(ed25519_dalek::SigningKey::from_bytes(&random::<32>())),
        rotated: Key::Ed(ed25519_dalek::SigningKey::from_bytes(&random::<32>())),
        attacker: Key::Ed(ed25519_dalek::SigningKey::from_bytes(&random::<32>())),
    })
}

impl Key {
    /// The public half as a JWK with this `kid`.
    pub fn jwk(&self, kid: &str) -> Value {
        match self {
            Key::Rsa(key) => {
                use rsa::traits::PublicKeyParts;
                json!({ "kty": "RSA", "kid": kid, "use": "sig", "n": b64(&key.n().to_be_bytes_trimmed_vartime()), "e": b64(&key.e().to_be_bytes_trimmed_vartime()) })
            }
            Key::P256(key) => {
                let point = key.verifying_key().to_sec1_point(false);
                json!({ "kty": "EC", "kid": kid, "crv": "P-256", "alg": "ES256", "x": b64(point.x().unwrap()), "y": b64(point.y().unwrap()) })
            }
            Key::Ed(key) => json!({ "kty": "OKP", "kid": kid, "crv": "Ed25519", "x": b64(key.verifying_key().as_bytes()) }),
        }
    }

    pub fn sign(&self, alg: &str, input: &[u8]) -> Vec<u8> {
        use rsa::sha2::Sha256;
        match (self, alg) {
            (Key::Rsa(key), "RS256") => rsa::pkcs1v15::SigningKey::<Sha256>::new(key.clone()).sign(input).to_vec(),
            (Key::Rsa(key), "PS256") => rsa::pss::SigningKey::<Sha256>::new(key.clone()).sign(input).to_vec(),
            (Key::P256(key), _) => { let s: p256::ecdsa::Signature = key.sign(input); s.to_bytes().to_vec() }
            (Key::Ed(key), _) => key.sign(input).to_bytes().to_vec(),
            _ => panic!("cannot sign {alg} with this key"),
        }
    }
}

/// The signing input of a JWS with this header and these claims.
pub fn signing_input(header: &Value, claims: &Value) -> String { format!("{}.{}", b64(header.to_string().as_bytes()), b64(claims.to_string().as_bytes())) }

/// HMAC-SHA256 (RFC 2104), for forging an `HS256` token keyed with the provider's public key.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut block = [0u8; 64];
    if key.len() > 64 { block[..32].copy_from_slice(&Sha256::digest(key)); } else { block[..key.len()].copy_from_slice(key); }
    let pad = |byte: u8| block.iter().map(|b| b ^ byte).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(message).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().to_vec()
}

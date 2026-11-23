use super::*;
use rsa::signature::{SignatureEncoding, Signer};
use serde_json::{Value, json};
use std::sync::OnceLock;

fn enc(bytes: &[u8]) -> String { base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes) }

fn random32() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).unwrap();
    bytes
}

/// One 2048-bit RSA key for every test here: generating it is the slow part.
fn rsa_key() -> &'static rsa::PrivateKey {
    static KEY: OnceLock<rsa::PrivateKey> = OnceLock::new();
    KEY.get_or_init(|| rsa::PrivateKey::new(&mut getrandom::SysRng, 2048).unwrap())
}

enum Signing { Rsa(&'static rsa::PrivateKey), P256(p256::ecdsa::SigningKey), P384(p384::ecdsa::SigningKey), Ed(ed25519_dalek::SigningKey) }

impl Signing {
    fn p256() -> Self { Signing::P256(p256::ecdsa::SigningKey::from_slice(&random32()).unwrap()) }
    fn p384() -> Self {
        let mut bytes = [0u8; 48];
        getrandom::fill(&mut bytes).unwrap();
        Signing::P384(p384::ecdsa::SigningKey::from_slice(&bytes).unwrap())
    }
    fn ed() -> Self { Signing::Ed(ed25519_dalek::SigningKey::from_bytes(&random32())) }

    /// The public half as a JWK, with these extra members (`kid`, `alg`, …).
    fn jwk(&self, extra: Value) -> Value {
        let mut jwk = match self {
            Signing::Rsa(key) => {
                use rsa::traits::PublicKeyParts;
                json!({ "kty": "RSA", "n": enc(&key.n().to_be_bytes_trimmed_vartime()), "e": enc(&key.e().to_be_bytes_trimmed_vartime()) })
            }
            Signing::P256(key) => {
                let point = key.verifying_key().to_sec1_point(false);
                json!({ "kty": "EC", "crv": "P-256", "x": enc(point.x().unwrap()), "y": enc(point.y().unwrap()) })
            }
            Signing::P384(key) => {
                let point = key.verifying_key().to_sec1_point(false);
                json!({ "kty": "EC", "crv": "P-384", "x": enc(point.x().unwrap()), "y": enc(point.y().unwrap()) })
            }
            Signing::Ed(key) => json!({ "kty": "OKP", "crv": "Ed25519", "x": enc(key.verifying_key().as_bytes()) }),
        };
        for (k, v) in extra.as_object().unwrap() { jwk[k] = v.clone(); }
        jwk
    }

    fn sign(&self, alg: &str, input: &[u8]) -> Vec<u8> {
        use rsa::sha2::{Sha256, Sha384, Sha512};
        match (self, alg) {
            (Signing::Rsa(key), "RS256") => rsa::pkcs1v15::SigningKey::<Sha256>::new((*key).clone()).sign(input).to_vec(),
            (Signing::Rsa(key), "RS384") => rsa::pkcs1v15::SigningKey::<Sha384>::new((*key).clone()).sign(input).to_vec(),
            (Signing::Rsa(key), "RS512") => rsa::pkcs1v15::SigningKey::<Sha512>::new((*key).clone()).sign(input).to_vec(),
            (Signing::Rsa(key), "PS256") => rsa::pss::SigningKey::<Sha256>::new((*key).clone()).sign(input).to_vec(),
            (Signing::P256(key), _) => { let s: p256::ecdsa::Signature = key.sign(input); s.to_bytes().to_vec() }
            (Signing::P384(key), _) => { let s: p384::ecdsa::Signature = key.sign(input); s.to_bytes().to_vec() }
            (Signing::Ed(key), _) => key.sign(input).to_bytes().to_vec(),
            _ => panic!("cannot sign {alg} with this key"),
        }
    }

    /// A compact JWS of `claims` with this `header`, signed with `alg`.
    fn token(&self, alg: &str, header: Value, claims: Value) -> String {
        let input = format!("{}.{}", enc(header.to_string().as_bytes()), enc(claims.to_string().as_bytes()));
        let signature = self.sign(alg, input.as_bytes());
        format!("{input}.{}", enc(&signature))
    }
}

fn set(keys: &[Value]) -> KeySet { parse_key_set(json!({ "keys": keys }).to_string().as_bytes()).unwrap() }

fn claims() -> Value { json!({ "iss": "https://idp.example", "sub": "u1" }) }

/// Each part of a token, so tests can rebuild it with one part changed.
fn parts(token: &str) -> Vec<String> { token.split('.').map(str::to_owned).collect() }

#[test]
fn every_allowed_algorithm_verifies_and_returns_the_payload() {
    let rsa = Signing::Rsa(rsa_key());
    let (p256, p384, ed) = (Signing::p256(), Signing::p384(), Signing::ed());
    let keys = set(&[rsa.jwk(json!({ "kid": "r", "use": "sig" })), p256.jwk(json!({ "kid": "e256" })), p384.jwk(json!({ "kid": "e384", "alg": "ES384" })), ed.jwk(json!({ "kid": "ed", "key_ops": ["verify"] }))]);
    assert_eq!(keys.len(), 4);
    for (signer, alg, kid) in [(&rsa, "RS256", "r"), (&rsa, "RS384", "r"), (&rsa, "RS512", "r"), (&rsa, "PS256", "r"), (&p256, "ES256", "e256"), (&p384, "ES384", "e384"), (&ed, "EdDSA", "ed")] {
        let token = signer.token(alg, json!({ "alg": alg, "kid": kid, "typ": "JWT" }), claims());
        let payload = verify(&token, &keys).unwrap_or_else(|e| panic!("{alg}: {e:?}"));
        assert_eq!(serde_json::from_slice::<Value>(&payload).unwrap(), claims(), "{alg}");
    }
}

#[test]
fn a_tampered_payload_or_signature_is_refused() {
    let ed = Signing::ed();
    let p256 = Signing::p256();
    let keys = set(&[ed.jwk(json!({ "kid": "ed" })), p256.jwk(json!({ "kid": "ec" }))]);
    for (signer, alg, kid) in [(&ed, "EdDSA", "ed"), (&p256, "ES256", "ec")] {
        let token = signer.token(alg, json!({ "alg": alg, "kid": kid }), claims());
        let p = parts(&token);
        let forged = enc(json!({ "iss": "https://idp.example", "sub": "admin" }).to_string().as_bytes());
        assert_eq!(verify(&format!("{}.{forged}.{}", p[0], p[2]), &keys), Err(JoseError::BadSignature), "{alg} payload");
        let mut signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&p[2]).unwrap();
        signature[10] ^= 1;
        assert_eq!(verify(&format!("{}.{}.{}", p[0], p[1], enc(&signature)), &keys), Err(JoseError::BadSignature), "{alg} signature");
        assert_eq!(verify(&format!("{}.{}.{}", p[0], p[1], enc(&signature[..40])), &keys), Err(JoseError::BadSignature), "{alg} short signature");
    }
}

#[test]
fn a_tampered_rsa_token_is_refused() {
    let rsa = Signing::Rsa(rsa_key());
    let keys = set(&[rsa.jwk(json!({ "kid": "r" }))]);
    for alg in ["RS256", "PS256"] {
        let p = parts(&rsa.token(alg, json!({ "alg": alg, "kid": "r" }), claims()));
        let forged = enc(json!({ "sub": "admin" }).to_string().as_bytes());
        assert_eq!(verify(&format!("{}.{forged}.{}", p[0], p[2]), &keys), Err(JoseError::BadSignature), "{alg}");
        // The same signature under the other padding scheme does not verify either.
        let other = if alg == "RS256" { "PS256" } else { "RS256" };
        let header = enc(json!({ "alg": other, "kid": "r" }).to_string().as_bytes());
        assert_eq!(verify(&format!("{header}.{}.{}", p[1], p[2]), &keys), Err(JoseError::BadSignature), "{alg} as {other}");
    }
}

#[test]
fn none_and_hmac_are_never_accepted() {
    let rsa = Signing::Rsa(rsa_key());
    let keys = set(&[rsa.jwk(json!({ "kid": "r" }))]);
    let body = enc(claims().to_string().as_bytes());
    for alg in ["none", "None", "NONE", "HS256", "HS384", "HS512", "PS512", "ES512", "RS1", ""] {
        let header = enc(json!({ "alg": alg, "kid": "r" }).to_string().as_bytes());
        assert_eq!(verify(&format!("{header}.{body}.{}", enc(b"anything")), &keys), Err(JoseError::Unsupported), "{alg}");
    }
    let header = enc(br#"{"alg":"none"}"#);
    assert!(verify(&format!("{header}.{body}."), &keys).is_err(), "an unsigned token");
    let header = enc(br#"{"kid":"r"}"#);
    assert_eq!(verify(&format!("{header}.{body}.{}", enc(b"x")), &keys), Err(JoseError::Malformed), "no alg");
}

#[test]
fn the_key_is_picked_by_kid_and_must_fit_the_alg() {
    let rsa = Signing::Rsa(rsa_key());
    let (ec, ed) = (Signing::p256(), Signing::ed());
    let keys = set(&[rsa.jwk(json!({ "kid": "r", "alg": "RS256" })), ec.jwk(json!({ "kid": "e" }))]);
    assert_eq!(verify(&ec.token("ES256", json!({ "alg": "ES256", "kid": "gone" }), claims()), &keys), Err(JoseError::UnknownKey));
    assert_eq!(verify(&ec.token("ES256", json!({ "alg": "ES256", "kid": "r" }), claims()), &keys), Err(JoseError::KeyMismatch), "an EC alg naming an RSA key");
    assert_eq!(verify(&rsa.token("PS256", json!({ "alg": "PS256", "kid": "r" }), claims()), &keys), Err(JoseError::KeyMismatch), "the key's own alg is RS256");
    assert_eq!(verify(&ed.token("EdDSA", json!({ "alg": "EdDSA", "kid": "e" }), claims()), &keys), Err(JoseError::KeyMismatch), "EdDSA naming a P-256 key");
    assert!(verify(&ec.token("ES256", json!({ "alg": "ES256" }), claims()), &keys).is_ok(), "no kid: the only P-256 key");
    assert_eq!(verify(&ed.token("EdDSA", json!({ "alg": "EdDSA" }), claims()), &keys), Err(JoseError::UnknownKey), "no kid and no Ed25519 key");
    let two = set(&[ec.jwk(json!({ "kid": "a" })), Signing::p256().jwk(json!({ "kid": "b" }))]);
    assert_eq!(verify(&ec.token("ES256", json!({ "alg": "ES256" }), claims()), &two), Err(JoseError::Ambiguous));
    assert!(verify(&ec.token("ES256", json!({ "alg": "ES256", "kid": "a" }), claims()), &two).is_ok());
}

#[test]
fn a_key_carried_or_pointed_to_by_the_token_is_ignored() {
    let (real, attacker) = (Signing::ed(), Signing::ed());
    let keys = set(&[real.jwk(json!({ "kid": "real" }))]);
    let attacker_jwk = attacker.jwk(json!({ "kid": "evil" }));
    for header in [
        json!({ "alg": "EdDSA", "kid": "real", "jwk": attacker_jwk }),
        json!({ "alg": "EdDSA", "kid": "real", "jku": "https://evil.example/jwks", "x5u": "https://evil.example/cert" }),
        json!({ "alg": "EdDSA", "jwk": attacker_jwk, "jku": "https://evil.example/jwks" }),
    ] {
        assert_eq!(verify(&attacker.token("EdDSA", header.clone(), claims()), &keys), Err(JoseError::BadSignature), "{header}");
    }
    let header = json!({ "alg": "EdDSA", "kid": "evil", "jwk": attacker_jwk, "jku": "https://evil.example/jwks" });
    assert_eq!(verify(&attacker.token("EdDSA", header, claims()), &keys), Err(JoseError::UnknownKey));
    let header = json!({ "alg": "EdDSA", "kid": "real", "crit": ["exp"], "exp": 1 });
    assert_eq!(verify(&real.token("EdDSA", header, claims()), &keys), Err(JoseError::Unsupported), "crit is refused");
}

#[test]
fn malformed_tokens_are_refused() {
    let ed = Signing::ed();
    let keys = set(&[ed.jwk(json!({ "kid": "k" }))]);
    let token = ed.token("EdDSA", json!({ "alg": "EdDSA", "kid": "k" }), claims());
    assert!(verify(&token, &keys).is_ok());
    let p = parts(&token);
    for bad in [
        format!("{}.{}", p[0], p[1]),
        format!("{token}.extra"),
        format!("{}=.{}.{}", p[0], p[1], p[2]),
        format!("{}.{}.{}=", p[0], p[1], p[2]),
        format!("{}.{}.{}", p[0].replace('e', "+"), p[1], p[2]),
        format!("{}.{}.{}", enc(b"not json"), p[1], p[2]),
        format!("{}.{}.{}", enc(br#"{"alg":"EdDSA","alg":"none"}"#), p[1], p[2]),
        String::new(),
    ] {
        assert!(verify(&bad, &keys).is_err(), "{bad}");
    }
}

#[test]
fn only_usable_signature_keys_enter_the_set() {
    let ec = Signing::p256();
    let good = ec.jwk(json!({ "kid": "ok" }));
    let mut off_curve = good.clone();
    off_curve["y"] = Value::String(enc(&[7u8; 32]));
    let mut short_x = good.clone();
    short_x["x"] = Value::String(enc(&[1u8; 31]));
    let weak_rsa = json!({ "kty": "RSA", "n": enc(&[0xffu8; 128]), "e": "AQAB" });
    let keys = set(&[
        good.clone(),
        ec.jwk(json!({ "use": "enc" })),
        ec.jwk(json!({ "key_ops": ["encrypt"] })),
        ec.jwk(json!({ "alg": "ECDH-ES" })),
        ec.jwk(json!({ "alg": "RS256" })),
        off_curve,
        short_x,
        weak_rsa,
        json!({ "kty": "oct", "k": enc(b"shared secret") }),
        json!({ "kty": "EC", "crv": "P-521", "x": "AA", "y": "AA" }),
        json!({ "kty": "OKP", "crv": "X25519", "x": enc(&[9u8; 32]) }),
        json!("not an object"),
    ]);
    assert_eq!(keys.len(), 1, "only the good key");
    assert!(parse_key_set(b"{}").is_none() && parse_key_set(b"[]").is_none() && parse_key_set(b"<html>").is_none());
    assert!(parse_key_set(br#"{"keys":[]}"#).unwrap().is_empty());
}

#[test]
fn reasons_never_carry_the_token() {
    for e in [JoseError::Malformed, JoseError::Unsupported, JoseError::UnknownKey, JoseError::KeyMismatch, JoseError::Ambiguous, JoseError::BadSignature] {
        assert!(e.reason().len() < 40 && !e.reason().contains('.'));
    }
}

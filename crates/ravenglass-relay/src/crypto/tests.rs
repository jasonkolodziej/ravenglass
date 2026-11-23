use super::*;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rg-relay-crypto-{name}-{}", random_token(6)));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn sealed_values_open_only_with_the_same_user_and_context() {
    let master = MasterKey::generate();
    let (alice, bob) = (master.user_key(1), master.user_key(2));
    let sealed = alice.seal("1/work/password", b"hunter2");
    assert!(!sealed.windows(7).any(|w| w == b"hunter2"));
    assert_eq!(alice.open("1/work/password", &sealed).unwrap(), b"hunter2");
    assert_eq!(bob.open("1/work/password", &sealed), Err(SealError), "another user's key");
    assert_eq!(alice.open("1/home/password", &sealed), Err(SealError), "moved to another row");
    let mut tampered = sealed.clone();
    *tampered.last_mut().unwrap() ^= 1;
    assert_eq!(alice.open("1/work/password", &tampered), Err(SealError));
    assert_eq!(alice.open("1/work/password", b"short"), Err(SealError));
    assert_ne!(alice.seal("c", b"x"), alice.seal("c", b"x"), "a fresh nonce every time");
}

#[test]
fn user_keys_are_stable_for_the_same_master_key() {
    let master = MasterKey::from_bytes([7; 32]);
    let sealed = master.user_key(5).seal("ctx", b"secret");
    assert_eq!(MasterKey::from_bytes([7; 32]).user_key(5).open("ctx", &sealed).unwrap(), b"secret");
    assert!(MasterKey::from_bytes([8; 32]).user_key(5).open("ctx", &sealed).is_err());
}

#[test]
fn the_master_key_file_is_created_once_private_and_then_reused() {
    let dir = temp_dir("create");
    let path = dir.join("keys/master.key");
    let (first, created) = MasterKey::load_or_create(&path).unwrap();
    assert!(created);
    assert_eq!(std::fs::read(&path).unwrap().len(), 32);
    assert!(!readable_by_others(&path), "created with mode 0600");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let (again, created) = MasterKey::load_or_create(&path).unwrap();
    assert!(!created);
    let sealed = first.user_key(1).seal("c", b"v");
    assert_eq!(again.user_key(1).open("c", &sealed).unwrap(), b"v");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_malformed_master_key_is_refused_without_echoing_it() {
    let dir = temp_dir("bad");
    let path = dir.join("master.key");
    std::fs::write(&path, b"not a key, but a secretish string\n").unwrap();
    let error = MasterKey::load_or_create(&path).err().unwrap();
    assert_eq!(error, KeyError::Malformed);
    assert!(!error.to_string().contains("secretish"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn passwords_hash_with_argon2id_and_a_fresh_salt() {
    let hash = hash_password("correct horse", PasswordCost::Weak);
    assert!(hash.starts_with("$argon2id$v=19$"), "{hash}");
    assert!(!hash.contains("correct horse"));
    assert_ne!(hash, hash_password("correct horse", PasswordCost::Weak));
    assert!(verify_password("correct horse", &hash));
    assert!(!verify_password("correct horsf", &hash));
    assert!(!verify_password("correct horse", "not a hash"));
    assert!(hash_password("x", PasswordCost::Standard).contains("m=19456,t=2,p=1"));
}

#[test]
fn tokens_are_random_and_digests_hide_them() {
    let (a, b) = (random_token(32), random_token(32));
    assert_ne!(a, b);
    assert_eq!(a.len(), 43);
    assert_eq!(digest(&a).len(), 64);
    assert!(!digest(&a).contains(&a));
    assert!(same(b"abc", b"abc") && !same(b"abc", b"abd") && !same(b"abc", b"ab"));
}

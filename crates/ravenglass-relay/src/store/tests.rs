use super::*;
use crate::crypto::random_token;
use mail_auth::{AuthKind, Incoming, Security, Server};

pub fn account(id: &str, address: &str) -> AccountConfig {
    AccountConfig {
        id: AccountId::from(id),
        name: "Jason".into(),
        address: address.into(),
        username: address.into(),
        incoming: Incoming::Imap(Server::new("imap.example.net", 993, Security::Tls)),
        smtp: Some(Server::new("smtp.example.net", 465, Security::Tls)),
        auth: AuthKind::Password,
        server_saves_sent: false,
    }
}

#[test]
fn the_database_file_holds_no_secret_address_or_account_id_in_clear() {
    let dir = std::env::temp_dir().join(format!("rg-relay-store-{}", random_token(6)));
    let path = dir.join("relay.sqlite3");
    {
        let store = Store::open(&path, MasterKey::from_bytes([3; 32])).unwrap();
        let user = store.add_user("alice", Some("$argon2id$fake")).unwrap();
        store.add_account(user.id, &account("work-account-id", "alice.secret@example.net")).unwrap();
        store.set_secret(user.id, &AccountId::from("work-account-id"), SecretKind::Password, "pw-correct-horse-99").unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    for needle in ["pw-correct-horse-99", "alice.secret@example.net", "work-account-id", "imap.example.net"] {
        assert!(!bytes.windows(needle.len()).any(|w| w == needle.as_bytes()), "{needle} is in the file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0, "private to its owner");
    }
    let reopened = Store::open(&path, MasterKey::from_bytes([3; 32])).unwrap();
    assert_eq!(reopened.secret(1, &AccountId::from("work-account-id"), SecretKind::Password).unwrap().as_deref(), Some("pw-correct-horse-99"));
    let wrong_key = Store::open(&path, MasterKey::from_bytes([4; 32])).unwrap();
    assert_eq!(wrong_key.accounts(1), Err(StoreError::Sealed), "a different master key opens nothing");
    // Windows will not delete a database file that is still open.
    drop((reopened, wrong_key));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn users_see_only_their_own_accounts_and_secrets() {
    let store = Arc::new(Store::open_in_memory(MasterKey::generate()).unwrap());
    let (alice, bob) = (store.add_user("alice", None).unwrap(), store.add_user("bob", None).unwrap());
    let shared_id = AccountId::from("work");
    store.add_account(alice.id, &account("work", "a@example.net")).unwrap();
    store.add_account(bob.id, &account("work", "b@example.net")).unwrap();
    assert_eq!(store.add_account(alice.id, &account("work", "again@example.net")), Err(StoreError::Conflict));
    let (alice_secrets, bob_secrets) = (UserSecrets::new(store.clone(), alice.id), UserSecrets::new(store.clone(), bob.id));
    alice_secrets.set(&shared_id, SecretKind::Password, "alice-pw").unwrap();
    assert_eq!(bob_secrets.get(&shared_id, SecretKind::Password).unwrap(), None);
    bob_secrets.set(&shared_id, SecretKind::Password, "bob-pw").unwrap();
    assert_eq!(alice_secrets.get(&shared_id, SecretKind::Password).unwrap().as_deref(), Some("alice-pw"));
    assert_eq!(store.accounts(alice.id).unwrap()[0].address, "a@example.net");
    assert_eq!(store.account(bob.id, &shared_id).unwrap().unwrap().address, "b@example.net");
    assert!(store.remove_account(alice.id, &shared_id).unwrap());
    assert!(!store.remove_account(alice.id, &shared_id).unwrap());
    assert_eq!(alice_secrets.get(&shared_id, SecretKind::Password).unwrap(), None, "secrets go with the account");
    assert_eq!(bob_secrets.get(&shared_id, SecretKind::Password).unwrap().as_deref(), Some("bob-pw"));
}

#[test]
fn removing_a_user_removes_everything_they_had() {
    let store = Store::open_in_memory(MasterKey::generate()).unwrap();
    let alice = store.add_user("alice", Some("hash")).unwrap();
    assert_eq!(store.add_user("alice", None), Err(StoreError::Conflict));
    store.add_account(alice.id, &account("work", "a@example.net")).unwrap();
    store.set_secret(alice.id, &AccountId::from("work"), SecretKind::Password, "pw").unwrap();
    assert_eq!(store.password_of("alice").unwrap().unwrap().1, "hash");
    assert_eq!(store.remove_user("alice").unwrap(), Some(alice.clone()));
    assert_eq!(store.remove_user("alice").unwrap(), None);
    assert!(store.users().unwrap().is_empty());
    assert!(store.accounts(alice.id).unwrap().is_empty());
    assert_eq!(store.secret(alice.id, &AccountId::from("work"), SecretKind::Password).unwrap(), None);
}

#[test]
fn oidc_users_are_created_once_per_subject_and_never_take_a_name() {
    let store = Store::open_in_memory(MasterKey::generate()).unwrap();
    store.add_user("a@example.org", Some("hash")).unwrap();
    let first = store.oidc_user("https://idp|123", "a@example.org").unwrap();
    assert_eq!(first.name, "a@example.org#1", "a local user keeps their name");
    assert_eq!(store.oidc_user("https://idp|123", "renamed@example.org").unwrap(), first);
    assert_ne!(store.oidc_user("https://idp|456", "x@example.org").unwrap().id, first.id);
    assert_eq!(store.password_of("a@example.org#1").unwrap(), None, "no password sign-in for OIDC users");
}

use super::*;
use mail_auth::{MemorySecrets, autoconfig};
use mail_core::AccountId;
use std::sync::Mutex;

fn account(email: &str) -> AccountConfig {
    autoconfig::discover(email, &|_| None).unwrap().into_account(AccountId::from("acc"), "Jason", email)
}

fn connector(post: Option<FormPost>) -> (Connector, Arc<MemorySecrets>) {
    let secrets = Arc::new(MemorySecrets::default());
    (Connector { secrets: secrets.clone(), client_ids: ClientIds { google: Some("client-1".into()), microsoft: None }, post }, secrets)
}

/// The `(url, body)` of each request a fake token endpoint received.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// A token endpoint that records what it was sent and answers from a script.
fn endpoint(answers: Vec<&'static str>) -> (FormPost, Seen) {
    let seen: Seen = Arc::default();
    let answers = Mutex::new(answers.into_iter());
    let post: FormPost = { let seen = seen.clone(); Arc::new(move |url: &str, body: &str| { seen.lock().unwrap().push((url.to_owned(), body.to_owned())); answers.lock().unwrap().next().map(str::to_owned).ok_or_else(|| "offline".to_owned()) }) };
    (post, seen)
}

#[test]
fn a_password_account_connects_with_its_keyring_secret() {
    let (c, secrets) = connector(None);
    let config = account("jason@example.org");
    assert!(matches!(c.connect(&config, 0.0), Err(ConnectError::NoPassword)));
    secrets.set(&config.id, SecretKind::Password, "app-password").unwrap();
    let connected = c.connect(&config, 0.0).unwrap();
    assert_eq!(connected.backend.account(), &config.id);
    assert_eq!((connected.identity.display(), connected.identity.email.as_str(), connected.token_expires_at), ("Jason", "jason@example.org", None));
    let unsafe_config = AccountConfig { smtp: Some(Server::new("smtp.example.org", 25, Security::Plain)), ..config };
    assert!(matches!(c.connect(&unsafe_config, 0.0), Err(ConnectError::Invalid(_))));
}

#[test]
fn an_oauth_account_trades_its_refresh_token_and_keeps_a_rotated_one() {
    let (post, seen) = endpoint(vec![r#"{"access_token":"at-1","expires_in":3600,"refresh_token":"rt-2"}"#, r#"{"access_token":"at-2","expires_in":3600}"#]);
    let (c, secrets) = connector(Some(post));
    let config = account("me@gmail.com");
    assert!(matches!(c.connect(&config, 0.0), Err(ConnectError::SignIn)), "no refresh token yet");
    secrets.set(&config.id, SecretKind::RefreshToken, "rt-1").unwrap();
    let mut connected = c.connect(&config, 100.0).unwrap();
    assert_eq!(connected.token_expires_at, Some(3700.0));
    assert_eq!(secrets.get(&config.id, SecretKind::RefreshToken).unwrap().as_deref(), Some("rt-2"));
    c.refresh(&mut connected, &config, 3600.0).unwrap();
    assert_eq!(connected.token_expires_at, Some(7200.0));
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0], ("https://oauth2.googleapis.com/token".to_owned(), "grant_type=refresh_token&client_id=client-1&refresh_token=rt-1".to_owned()));
    assert!(seen[1].1.ends_with("refresh_token=rt-2"));
}

#[test]
fn a_dead_refresh_token_is_forgotten_and_asks_for_sign_in() {
    let (post, _) = endpoint(vec![r#"{"error":"invalid_grant"}"#]);
    let (c, secrets) = connector(Some(post));
    let config = account("me@gmail.com");
    secrets.set(&config.id, SecretKind::RefreshToken, "revoked").unwrap();
    assert!(matches!(c.connect(&config, 0.0), Err(ConnectError::SignIn)));
    assert_eq!(secrets.get(&config.id, SecretKind::RefreshToken).unwrap(), None);
    // Offline is not the same as revoked: the token is kept.
    secrets.set(&config.id, SecretKind::RefreshToken, "fine").unwrap();
    assert!(matches!(c.connect(&config, 0.0), Err(ConnectError::Network(_))));
    assert!(secrets.get(&config.id, SecretKind::RefreshToken).unwrap().is_some());
    assert!(matches!(c.connect(&account("me@outlook.com"), 0.0), Err(ConnectError::NoClientId(Provider::Microsoft))));
}

#[test]
fn completing_a_browser_sign_in_stores_only_the_refresh_token() {
    let (post, seen) = endpoint(vec![r#"{"access_token":"at","expires_in":3600,"refresh_token":"rt-new"}"#]);
    let (c, secrets) = connector(Some(post));
    let config = account("me@gmail.com");
    let attempt = oauth::Attempt::new().unwrap();
    let query = format!("code=abc&state={}", attempt.state);
    assert!(matches!(c.complete_sign_in(&config, &attempt, "http://127.0.0.1:1/", "code=abc&state=forged", 0.0), Err(ConnectError::OAuth(OAuthError::StateMismatch))));
    c.complete_sign_in(&config, &attempt, "http://127.0.0.1:1/", &query, 0.0).unwrap();
    assert_eq!(secrets.get(&config.id, SecretKind::RefreshToken).unwrap().as_deref(), Some("rt-new"));
    assert_eq!(secrets.get(&config.id, SecretKind::Password).unwrap(), None);
    assert!(seen.lock().unwrap()[0].1.starts_with("grant_type=authorization_code&client_id=client-1"));
}

#[test]
fn a_jmap_account_gets_a_jmap_backend() {
    let (c, secrets) = connector(None);
    let config = account("me@fastmail.com");
    secrets.set(&config.id, SecretKind::Password, "app-password").unwrap();
    assert_eq!(c.connect(&config, 0.0).unwrap().backend.account(), &config.id);
}

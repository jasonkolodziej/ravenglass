use super::*;
use std::collections::HashMap;

fn load_from(vars: &[(&str, &str)]) -> Result<Settings, String> {
    let map: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    load(&move |name: &str| map.get(name).cloned())
}

#[test]
fn token_mode_is_the_default_and_needs_a_token() {
    assert!(load_from(&[]).err().unwrap().contains("RAVENGLASS_RELAY_TOKEN"));
    let settings = load_from(&[("RAVENGLASS_RELAY_TOKEN", "0123456789abcdef"), ("RAVENGLASS_RELAY_ORIGIN", "https://mail.example.org/")]).unwrap();
    assert!(matches!(settings.mode, ModeSettings::Token { ref token } if token == "0123456789abcdef"));
    assert_eq!(settings.listen, "127.0.0.1:8743");
    assert_eq!(settings.origin.as_deref(), Some("https://mail.example.org"));
}

#[test]
fn users_mode_from_the_environment_alone() {
    let settings = load_from(&[
        ("RAVENGLASS_RELAY_AUTH", "users"),
        ("RAVENGLASS_RELAY_DATA_DIR", "/var/lib/relay"),
        ("RAVENGLASS_RELAY_OIDC_ISSUER", "https://idp.example"),
        ("RAVENGLASS_RELAY_OIDC_CLIENT_ID", "relay"),
        ("RAVENGLASS_RELAY_PUBLIC_URL", "https://relay.example.org/"),
        ("RAVENGLASS_RELAY_OIDC_ALLOWED_DOMAINS", "Example.org, example.net"),
        ("RAVENGLASS_RELAY_TRUST_FORWARDED_FOR", "1"),
    ]).unwrap();
    let ModeSettings::Users(users) = settings.mode else { panic!() };
    assert_eq!(users.database, PathBuf::from("/var/lib/relay/relay.sqlite3"));
    assert_eq!(users.master_key_file, PathBuf::from("/var/lib/relay/master.key"));
    assert!(users.password_login && users.trust_forwarded_for && !users.insecure_dev_cookies);
    let oidc = users.oidc.unwrap();
    assert_eq!(oidc.redirect_uri, "https://relay.example.org/v1/auth/oidc/callback");
    assert_eq!(oidc.allowed_domains, ["example.org", "example.net"]);
    assert_eq!(oidc.scopes, "openid email profile");
}

#[test]
fn a_toml_file_is_read_and_the_environment_wins() {
    let dir = std::env::temp_dir().join(format!("rg-relay-config-{}", crate::crypto::random_token(6)));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("relay.toml");
    let secret = dir.join("secret");
    std::fs::write(&secret, "file-secret-value\n").unwrap();
    std::fs::write(&path, format!("auth = \"users\"\nlisten = \"0.0.0.0:1\"\npassword_login = false\npublic_url = \"https://r.example\"\n[oidc]\nissuer = \"https://idp.example\"\nclient_id = \"c\"\nclient_secret_file = {:?}\nallowed_emails = [\"A@example.org\"]\n", secret.to_str().unwrap())).unwrap();
    let settings = load_from(&[("RAVENGLASS_RELAY_CONFIG", path.to_str().unwrap()), ("RAVENGLASS_RELAY_LISTEN", "127.0.0.1:2")]).unwrap();
    assert_eq!(settings.listen, "127.0.0.1:2");
    let ModeSettings::Users(users) = settings.mode else { panic!() };
    assert!(!users.password_login);
    let oidc = users.oidc.unwrap();
    assert_eq!(oidc.client_secret.as_deref(), Some("file-secret-value"));
    assert_eq!(oidc.allowed_emails, ["a@example.org"]);
    std::fs::write(&path, "unknown_key = 1\n").unwrap();
    assert!(load_from(&[("RAVENGLASS_RELAY_CONFIG", path.to_str().unwrap())]).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bad_settings_are_refused_with_a_reason() {
    assert!(load_from(&[("RAVENGLASS_RELAY_AUTH", "magic")]).is_err());
    assert!(load_from(&[("RAVENGLASS_RELAY_AUTH", "users"), ("RAVENGLASS_RELAY_PASSWORD_LOGIN", "false")]).err().unwrap().contains("OIDC"));
    assert!(load_from(&[("RAVENGLASS_RELAY_AUTH", "users"), ("RAVENGLASS_RELAY_OIDC_ISSUER", "http://idp.example"), ("RAVENGLASS_RELAY_OIDC_CLIENT_ID", "c"), ("RAVENGLASS_RELAY_PUBLIC_URL", "https://r")]).err().unwrap().contains("https"));
    assert!(load_from(&[("RAVENGLASS_RELAY_TOKEN", "x"), ("RAVENGLASS_RELAY_DEMO", "maybe")]).is_err());
    let missing = load_from(&[("RAVENGLASS_RELAY_TOKEN_FILE", "/nonexistent/token")]).err().unwrap();
    assert!(missing.contains("TOKEN_FILE"));
}

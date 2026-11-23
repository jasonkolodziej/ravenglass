use crate::autoconfig::{self, AutoconfigError, Source};
use crate::config::*;
use crate::oauth::{self, Attempt, OAuthError, Provider};
use crate::secrets::*;
use mail_core::AccountId;

const ISPDB: &str = r#"<?xml version="1.0"?><clientConfig version="1.1"><emailProvider id="example.org">
  <domain>example.org</domain>
  <incomingServer type="pop3"><hostname>pop.example.org</hostname><port>995</port><socketType>SSL</socketType></incomingServer>
  <incomingServer type="imap"><hostname>imap.%EMAILDOMAIN%</hostname><port>143</port><socketType>STARTTLS</socketType><username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication></incomingServer>
  <outgoingServer type="smtp"><hostname>smtp.example.org</hostname><port>465</port><socketType>SSL</socketType><username>%EMAILADDRESS%</username></outgoingServer>
</emailProvider></clientConfig>"#;

#[test]
fn known_providers_need_no_lookup() {
    let never = |_: &str| -> Option<String> { panic!("built-in providers must not be looked up") };
    let gmail = autoconfig::discover("me@gmail.com", &never).unwrap();
    assert_eq!((gmail.source, &gmail.auth, gmail.server_saves_sent), (Source::Builtin, &AuthKind::OAuth2(Provider::Google), true));
    let icloud = autoconfig::discover(" Me@iCloud.com ", &never).unwrap();
    assert_eq!(icloud.incoming, Incoming::Imap(Server::new("imap.mail.me.com", 993, Security::Tls)));
    assert_eq!((icloud.auth, icloud.username.as_str()), (AuthKind::Password, "Me@iCloud.com"));
    // Fastmail's JMAP refuses passwords, so a password account uses IMAP.
    assert_eq!(autoconfig::discover("me@fastmail.com", &never).unwrap().incoming, Incoming::Imap(Server::new("imap.fastmail.com", 993, Security::Tls)));
}

#[test]
fn custom_domains_use_ispdb_then_fall_back_to_a_guess() {
    let asked = std::cell::RefCell::new(Vec::new());
    let fetch = |url: &str| { asked.borrow_mut().push(url.to_owned()); url.contains("thunderbird.net").then(|| ISPDB.to_owned()) };
    let found = autoconfig::discover("jason@example.org", &fetch).unwrap();
    assert_eq!(found.source, Source::Ispdb);
    assert_eq!(found.incoming, Incoming::Imap(Server::new("imap.example.org", 143, Security::StartTls)));
    assert_eq!((found.smtp, found.username.as_str()), (Some(Server::new("smtp.example.org", 465, Security::Tls)), "jason"));
    let asked = asked.into_inner();
    assert_eq!(asked.len(), 3, "the provider's own locations are tried first");
    assert!(asked[0].contains("jason%40example.org") && !asked[2].contains("jason"), "Mozilla is asked by domain only: {asked:?}");

    let guessed = autoconfig::discover("jason@nowhere.example", &|_| Some("<not xml".into())).unwrap();
    assert_eq!((guessed.source, guessed.incoming), (Source::Guess, Incoming::Imap(Server::new("imap.nowhere.example", 993, Security::Tls))));
    assert_eq!(autoconfig::discover("not-an-address", &|_| None), Err(AutoconfigError::NotAnAddress));
    assert_eq!(autoconfig::parse_ispdb("<clientConfig/>", "a@b.example"), Err(AutoconfigError::NoImap));
}

#[test]
fn account_config_round_trips_and_rejects_unsafe_settings() {
    let account = autoconfig::guess("jason@example.org").unwrap().into_account(AccountId::from("work"), "Jason", "jason@example.org");
    assert_eq!(account.problem(), None);
    let json = serde_json::to_string(&account).unwrap();
    assert_eq!(serde_json::from_str::<AccountConfig>(&json).unwrap(), account);
    let plain = |host: &str| AccountConfig { incoming: Incoming::Imap(Server::new(host, 143, Security::Plain)), ..account.clone() };
    assert!(plain("mail.example.org").problem().is_some(), "no plain connections off this machine");
    assert_eq!(plain("127.0.0.1").problem(), None);
    assert!(AccountConfig { smtp: None, ..account.clone() }.problem().is_some());
    assert!(AccountConfig { incoming: Incoming::Jmap { session_url: "http://x.example/jmap".into() }, ..account }.problem().is_some());
}

#[test]
fn oauth_attempt_builds_the_pkce_flow_and_checks_the_redirect() {
    let (a, b) = (Attempt::new().unwrap(), Attempt::new().unwrap());
    assert_ne!(a.state, b.state);
    let url = a.authorize_url(Provider::Google, "client-1", "http://127.0.0.1:4000/", "me@gmail.com");
    assert!(url.starts_with("https://accounts.google.com/") && url.contains("code_challenge_method=S256") && url.contains("access_type=offline"));
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A4000%2F") && url.contains("scope=https%3A%2F%2Fmail.google.com%2F") && url.contains(&format!("state={}", a.state)));
    assert!(a.authorize_url(Provider::Microsoft, "c", "http://127.0.0.1:1/", "me@outlook.com").contains("offline_access"));

    assert_eq!(a.code(&format!("code=4%2Fabc+d&state={}", a.state)), Ok("4/abc d".into()));
    assert_eq!(a.code(&format!("code=x&state={}", b.state)), Err(OAuthError::StateMismatch));
    assert_eq!(a.code("error=access_denied&state=x"), Err(OAuthError::Denied("access_denied".into())));
    assert!(a.token_request("client-1", "http://127.0.0.1:4000/", "4/abc").contains("code=4%2Fabc&code_verifier="));
    assert!(!url.contains(a.token_request("c", "r", "x").rsplit('=').next().unwrap()), "the verifier never appears in the browser URL");
}

#[test]
fn token_answers_expire_refresh_and_never_print() {
    let t = oauth::parse_tokens(r#"{"access_token":"ya29.secret","refresh_token":"1//refresh","expires_in":3599,"token_type":"Bearer"}"#, 1000.0).unwrap();
    assert_eq!((t.access.as_str(), t.refresh.as_deref(), t.expires_at), ("ya29.secret", Some("1//refresh"), 4599.0));
    assert!(!t.needs_refresh(4000.0) && t.needs_refresh(4540.0));
    assert!(!format!("{t:?}").contains("secret") && !format!("{t:?}").contains("1//"));
    assert_eq!(oauth::parse_tokens(r#"{"error":"invalid_grant"}"#, 0.0), Err(OAuthError::Expired));
    assert!(matches!(oauth::parse_tokens(r#"{"error":"invalid_client"}"#, 0.0), Err(OAuthError::Denied(_))));
    assert!(matches!(oauth::parse_tokens("<html>", 0.0), Err(OAuthError::BadResponse(_))));
    assert_eq!(oauth::refresh_request("c 1", "1//r"), "grant_type=refresh_token&client_id=c%201&refresh_token=1%2F%2Fr");
}

#[test]
fn loopback_redirect_returns_the_query_and_ignores_other_requests() {
    use std::io::{Read, Write};
    let redirect = oauth::loopback::Redirect::listen().unwrap();
    let uri = redirect.uri().unwrap();
    let address = uri.trim_start_matches("http://").trim_end_matches('/').to_owned();
    let browser = std::thread::spawn(move || {
        let get = |path: &str| {
            let mut s = std::net::TcpStream::connect(&address).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut answer = String::new();
            s.read_to_string(&mut answer).unwrap();
            answer
        };
        assert!(get("/favicon.ico").starts_with("HTTP/1.1 404"));
        assert!(get("/?code=abc&state=xyz").contains("Signed in"));
    });
    assert_eq!(redirect.wait().unwrap(), "code=abc&state=xyz");
    browser.join().unwrap();
}

#[test]
fn memory_secrets_are_per_account_and_kind() {
    let store = MemorySecrets::default();
    let (a, b) = (AccountId::from("a"), AccountId::from("b"));
    store.set(&a, SecretKind::Password, "hunter2").unwrap();
    store.set(&a, SecretKind::RefreshToken, "1//r").unwrap();
    assert_eq!(store.get(&a, SecretKind::Password).unwrap().as_deref(), Some("hunter2"));
    assert_eq!(store.get(&b, SecretKind::Password).unwrap(), None);
    store.delete_all(&a).unwrap();
    assert_eq!((store.get(&a, SecretKind::Password).unwrap(), store.get(&a, SecretKind::RefreshToken).unwrap()), (None, None));
}

#[test]
fn a_passcode_is_stored_only_as_a_salted_hash_and_checks_exactly() {
    use crate::passcode;
    let store = MemorySecrets::default();
    assert!(!passcode::is_set(&store).unwrap() && !passcode::check(&store, "anything"));
    assert_eq!(passcode::set(&store, "12345"), Err(passcode::PasscodeError::TooShort));
    passcode::set(&store, "ravens 7").unwrap();
    let stored = store.get(&passcode::store_id(), SecretKind::Password).unwrap().unwrap();
    assert!(stored.starts_with("$argon2id$") && !stored.contains("ravens"), "{stored}");
    assert!(passcode::check(&store, "ravens 7"));
    for wrong in ["ravens7", "Ravens 7", "ravens 7 ", ""] { assert!(!passcode::check(&store, wrong), "{wrong:?}"); }
    // A fresh salt each time: the same passcode never hashes the same way twice.
    assert_ne!(passcode::hash("ravens 7").unwrap(), passcode::hash("ravens 7").unwrap());
    assert!(!passcode::verify("ravens 7", "not a hash"));
    // No account id can collide with the passcode's place in the store.
    assert!(passcode::store_id().0.contains(':'));
    passcode::clear(&store).unwrap();
    assert!(!passcode::is_set(&store).unwrap() && !passcode::check(&store, "ravens 7"));
}

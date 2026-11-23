use super::*;

fn provider() -> Provider {
    Provider {
        issuer: "https://idp.example".into(),
        authorization_endpoint: "https://idp.example/authorize".into(),
        token_endpoint: "https://idp.example/token".into(),
        userinfo_endpoint: "https://idp.example/userinfo".into(),
        jwks_uri: Some("https://idp.example/jwks".into()),
    }
}

fn client() -> Client {
    Client { client_id: "relay".into(), client_secret: None, redirect_uri: "https://relay.example/v1/auth/oidc/callback".into(), scopes: "openid email".into(), allowed_emails: vec![], allowed_domains: vec![] }
}

fn jwt(claims: serde_json::Value) -> String {
    let enc = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    format!("{}.{}.{}", enc(br#"{"alg":"RS256"}"#), enc(claims.to_string().as_bytes()), enc(b"sig"))
}

fn started() -> (State, String, String, String) {
    let (state, url) = start(&provider(), &client(), 1000);
    let State::AwaitingCallback { state: s, nonce, binding, .. } = &state else { panic!() };
    let (s, nonce, binding) = (s.clone(), nonce.clone(), binding.clone());
    assert!(url.starts_with("https://idp.example/authorize?response_type=code&client_id=relay&"), "{url}");
    assert!(url.contains(&format!("state={s}")) && url.contains(&format!("nonce={nonce}")) && url.contains("code_challenge_method=S256"));
    (state, s, nonce, binding)
}

fn tokens(nonce: &str, overrides: serde_json::Value) -> String {
    let mut claims = serde_json::json!({ "iss": "https://idp.example", "sub": "u1", "aud": "relay", "exp": 5000, "nonce": nonce });
    for (k, v) in overrides.as_object().unwrap() { claims[k] = v.clone(); }
    serde_json::json!({ "access_token": "at", "token_type": "Bearer", "id_token": jwt(claims) }).to_string()
}

fn through_tokens(overrides: serde_json::Value) -> (State, Option<Effect>) { through_signature(overrides, true) }

fn through_signature(overrides: serde_json::Value, valid: bool) -> (State, Option<Effect>) {
    let (state, s, nonce, binding) = started();
    let (state, effect) = state.step(&provider(), &client(), Event::Callback { query: format!("code=abc&state={s}"), binding, now: 1001 });
    let Some(Effect::PostToken { body, .. }) = effect else { panic!("{effect:?}") };
    assert!(body.contains("grant_type=authorization_code") && body.contains("code=abc") && body.contains("code_verifier=") && body.contains("client_id=relay"));
    let (state, effect) = state.step(&provider(), &client(), Event::Tokens { status: 200, body: tokens(&nonce, overrides), now: 1002 });
    let Some(Effect::CheckSignature { id_token }) = effect else { panic!("{effect:?}") };
    assert_eq!(id_token.split('.').count(), 3);
    assert!(matches!(&state, State::AwaitingSignature { now: 1002, .. }));
    state.step(&provider(), &client(), Event::Signature { valid })
}

#[test]
fn claims_are_read_only_after_the_signature_verifies() {
    assert_eq!(through_signature(serde_json::json!({}), false).1, Some(Effect::Fail(BAD_SIGNATURE)));
    assert_eq!(through_signature(serde_json::json!({ "iss": "https://evil.example", "exp": 1 }), false).1, Some(Effect::Fail(BAD_SIGNATURE)), "a bad signature hides every claim check");
    let (state, _) = through_tokens(serde_json::json!({}));
    assert_eq!(state.step(&provider(), &client(), Event::Signature { valid: true }).1, Some(Effect::Fail(OidcError::WrongState)), "one answer per check");
}

#[test]
fn a_full_sign_in_identifies_the_user() {
    let (state, effect) = through_tokens(serde_json::json!({}));
    assert_eq!(effect, Some(Effect::GetUserInfo { url: "https://idp.example/userinfo".into(), access_token: "at".into() }));
    let (state, effect) = state.step(&provider(), &client(), Event::UserInfo { status: 200, body: r#"{"sub":"u1","email":"a@example.org","email_verified":true}"#.into() });
    let identity = Identity { subject: "https://idp.example|u1".into(), email: Some("a@example.org".into()), name: "a@example.org".into() };
    assert_eq!(effect, Some(Effect::SignIn(identity.clone())));
    assert_eq!(state, State::SignedIn(identity));
}

#[test]
fn a_wrong_state_or_another_browser_is_refused() {
    let (state, s, _, binding) = started();
    let (_, effect) = state.clone().step(&provider(), &client(), Event::Callback { query: "code=abc&state=forged".into(), binding: binding.clone(), now: 1001 });
    assert_eq!(effect, Some(Effect::Fail(OidcError::WrongState)));
    let (_, effect) = state.clone().step(&provider(), &client(), Event::Callback { query: format!("code=abc&state={s}"), binding: "other-browser".into(), now: 1001 });
    assert_eq!(effect, Some(Effect::Fail(OidcError::WrongState)));
    let (_, effect) = state.step(&provider(), &client(), Event::Callback { query: format!("code=abc&state={s}"), binding, now: 1000 + FLOW_LIFETIME });
    assert_eq!(effect, Some(Effect::Fail(OidcError::Expired)));
}

#[test]
fn id_tokens_with_a_wrong_nonce_issuer_audience_or_expiry_are_refused() {
    assert_eq!(through_tokens(serde_json::json!({ "nonce": "replayed" })).1, Some(Effect::Fail(OidcError::WrongNonce)));
    assert_eq!(through_tokens(serde_json::json!({ "nonce": null })).1, Some(Effect::Fail(OidcError::WrongNonce)));
    assert_eq!(through_tokens(serde_json::json!({ "iss": "https://evil.example" })).1, Some(Effect::Fail(OidcError::Token("wrong issuer"))));
    assert_eq!(through_tokens(serde_json::json!({ "aud": "someone-else" })).1, Some(Effect::Fail(OidcError::Token("not issued to this relay"))));
    assert_eq!(through_tokens(serde_json::json!({ "aud": ["relay", "x"] })).1, Some(Effect::Fail(OidcError::Token("wrong authorized party"))));
    assert!(matches!(through_tokens(serde_json::json!({ "aud": ["relay", "x"], "azp": "relay" })).1, Some(Effect::GetUserInfo { .. })));
    assert_eq!(through_tokens(serde_json::json!({ "exp": 100 })).1, Some(Effect::Fail(OidcError::Token("expired"))));
}

#[test]
fn userinfo_must_name_the_same_subject_and_pass_the_allowlist() {
    let restricted = Client { allowed_domains: vec!["example.org".into()], allowed_emails: vec!["boss@elsewhere.net".into()], ..client() };
    let user_info = |body: &str| {
        let (state, _) = through_tokens(serde_json::json!({}));
        state.step(&provider(), &restricted, Event::UserInfo { status: 200, body: body.into() }).1
    };
    assert_eq!(user_info(r#"{"sub":"u2","email":"a@example.org","email_verified":true}"#), Some(Effect::Fail(OidcError::UserInfo("a different subject than the ID token"))));
    assert_eq!(user_info(r#"{"sub":"u1","email":"a@evil.org","email_verified":true}"#), Some(Effect::Fail(OidcError::NotAllowed)));
    assert_eq!(user_info(r#"{"sub":"u1","email":"a@example.org","email_verified":false}"#), Some(Effect::Fail(OidcError::NotAllowed)), "unverified email");
    assert_eq!(user_info(r#"{"sub":"u1"}"#), Some(Effect::Fail(OidcError::NotAllowed)), "no email at all");
    assert!(matches!(user_info(r#"{"sub":"u1","email":"A@Example.org","email_verified":true}"#), Some(Effect::SignIn(_))));
    assert!(matches!(user_info(r#"{"sub":"u1","email":"boss@elsewhere.net","email_verified":true}"#), Some(Effect::SignIn(_))));
}

#[test]
fn provider_errors_and_secret_clients_are_handled() {
    let (state, s, _, binding) = started();
    let (_, effect) = state.step(&provider(), &client(), Event::Callback { query: format!("error=access_denied&state={s}"), binding, now: 1001 });
    assert_eq!(effect, Some(Effect::Fail(OidcError::Provider("access_denied".into()))));
    let confidential = Client { client_secret: Some("s3cret".into()), ..client() };
    let (state, url) = start(&provider(), &confidential, 0);
    assert!(!url.contains("s3cret"));
    let State::AwaitingCallback { state: s, binding, .. } = &state else { panic!() };
    let event = Event::Callback { query: format!("code=c&state={s}"), binding: binding.clone(), now: 1 };
    let Some(Effect::PostToken { body, basic, .. }) = state.clone().step(&provider(), &confidential, event).1 else { panic!() };
    assert!(!body.contains("s3cret") && !body.contains("client_id"));
    assert_eq!(basic.as_deref(), Some(base64::engine::general_purpose::STANDARD.encode("relay:s3cret").as_str()));
}

#[test]
fn discovery_must_name_the_configured_issuer() {
    let doc = r#"{"issuer":"https://idp.example","authorization_endpoint":"https://idp.example/a","token_endpoint":"https://idp.example/t","userinfo_endpoint":"https://idp.example/u"}"#;
    assert_eq!(parse_discovery(doc, "https://idp.example").unwrap().token_endpoint, "https://idp.example/t");
    assert!(parse_discovery(doc, "https://other.example").is_err());
    assert!(parse_discovery("{}", "https://idp.example").is_err());
    assert_eq!(parse_discovery(doc, "https://idp.example").unwrap().jwks_uri, None);
    let with_keys = doc.replace(r#""userinfo_endpoint""#, r#""jwks_uri":"https://idp.example/k","userinfo_endpoint""#);
    assert_eq!(parse_discovery(&with_keys, "https://idp.example").unwrap().jwks_uri.as_deref(), Some("https://idp.example/k"));
    let plain_keys = with_keys.replace("https://idp.example/k", "http://idp.example/k");
    assert_eq!(parse_discovery(&plain_keys, "https://idp.example"), Err(OidcError::Discovery("every endpoint must be https")));
    let plain = doc.replace("https://idp.example/t", "http://idp.example/t");
    assert_eq!(parse_discovery(&plain, "https://idp.example"), Err(OidcError::Discovery("every endpoint must be https")));
    assert!(secure_endpoint("http://127.0.0.1:9/token") && secure_endpoint("http://localhost/x") && !secure_endpoint("ftp://idp.example/"));
    assert_eq!(discovery_url("https://idp.example/realms/x/"), "https://idp.example/realms/x/.well-known/openid-configuration");
}

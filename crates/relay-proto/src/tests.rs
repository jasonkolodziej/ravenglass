use super::*;

#[test]
fn paths_round_trip_ids_with_slashes_spaces_and_unicode() {
    let (a, f, m) = (AccountId::from("me@example.com"), FolderId::from("Receipts/2026 ✓"), MessageId::from("INBOX/42%"));
    assert_eq!(route(Method::Get, &paths::folders(&a)), Some(Route::Folders(a.clone())));
    assert_eq!(route(Method::Post, &paths::sync(&a, &f)), Some(Route::Sync(a.clone(), f)));
    assert_eq!(route(Method::Get, &paths::raw(&a, &m)), Some(Route::Raw(a.clone(), m)));
    assert_eq!(route(Method::Post, &paths::ops(&a)), Some(Route::Ops(a.clone())));
    assert_eq!(route(Method::Post, &paths::send(&a)), Some(Route::Send(a.clone())));
    assert_eq!(route(Method::Post, paths::SESSION), Some(Route::Session));
}

#[test]
fn unknown_routes_wrong_methods_and_bad_escapes_are_rejected() {
    let a = AccountId::from("a");
    assert_eq!(route(Method::Post, &paths::folders(&a)), None);
    assert_eq!(route(Method::Get, "/v2/accounts/a/folders"), None);
    assert_eq!(route(Method::Get, "/v1/accounts/%zz/folders"), None);
    assert_eq!(route(Method::Get, "/v1/accounts/%ff/folders"), None);
    assert_eq!(route(Method::Get, "/v1/accounts/a/folders/extra"), None);
}

#[test]
fn versions_negotiate_to_the_newest_shared_one() {
    assert_eq!(negotiate(&[1, 2, 3]), Some(1));
    assert_eq!(negotiate(&[7]), None);
}

#[test]
fn errors_keep_their_meaning_across_the_wire() {
    for e in [MailError::Auth, MailError::Network("reset".into()), MailError::NotFound("x".into())] {
        let json = serde_json::to_string(&ApiError::from(e.clone())).unwrap();
        assert_eq!(MailError::from(serde_json::from_str::<ApiError>(&json).unwrap()), e);
    }
    assert_eq!(ErrorCode::Auth.http_status(), 401);
}

#[test]
fn push_carries_folders_only() {
    let push = Push::from(IdleEvent::Exists(FolderId::from("a/Inbox")));
    assert_eq!(serde_json::to_string(&push).unwrap(), r#"{"Exists":{"folder":"a/Inbox"}}"#);
}

use super::*;

#[test]
fn cache_control_sets_the_lifetime_within_bounds() {
    assert_eq!(lifetime(None), DEFAULT_LIFETIME);
    assert_eq!(lifetime(Some("public, max-age=7200, must-revalidate")), 7200);
    assert_eq!(lifetime(Some("Max-Age=\"600\"")), 600);
    assert_eq!(lifetime(Some("max-age=5")), MIN_LIFETIME, "too short");
    assert_eq!(lifetime(Some("max-age=0")), MIN_LIFETIME);
    assert_eq!(lifetime(Some("max-age=31536000")), MAX_LIFETIME, "too long");
    assert_eq!(lifetime(Some("no-store")), MIN_LIFETIME);
    assert_eq!(lifetime(Some("max-age=3600, no-cache")), MIN_LIFETIME);
    assert_eq!(lifetime(Some("max-age=soon")), DEFAULT_LIFETIME);
    assert_eq!(lifetime(Some("private")), DEFAULT_LIFETIME);
}

#[test]
fn a_key_set_is_fetched_once_used_until_it_expires_then_fetched_again() {
    let mut cache = KeyCache::default();
    assert_eq!(cache.plan(1000), Plan::Fetch);
    cache.fetched(KeySet::default(), 600, 1000);
    assert_eq!(cache.plan(1000), Plan::Use);
    assert_eq!(cache.plan(1599), Plan::Use);
    assert_eq!(cache.plan(1600), Plan::Fetch, "expired");
    cache.failed(1600);
    assert_eq!(cache.plan(1601), Plan::Use, "the stale set while a refetch is held back");
    assert_eq!(cache.plan(1600 + MIN_REFETCH), Plan::Fetch);
}

#[test]
fn an_unknown_key_refetches_at_most_once_a_minute() {
    let mut cache = KeyCache::default();
    cache.fetched(KeySet::default(), 3600, 1000);
    assert_eq!(cache.plan_unknown_key(1000), Plan::Refuse, "just fetched");
    assert_eq!(cache.plan_unknown_key(1000 + MIN_REFETCH), Plan::Fetch);
    cache.fetched(KeySet::default(), 3600, 1000 + MIN_REFETCH);
    for t in 0..MIN_REFETCH { assert_eq!(cache.plan_unknown_key(1000 + MIN_REFETCH + t), Plan::Refuse); }
}

#[test]
fn with_no_key_set_and_a_recent_failure_sign_in_is_refused() {
    let mut cache = KeyCache::default();
    cache.failed(1000);
    assert_eq!(cache.plan(1001), Plan::Refuse);
    assert!(cache.keys().is_none());
    assert_eq!(cache.plan(1000 + MIN_REFETCH), Plan::Fetch);
}

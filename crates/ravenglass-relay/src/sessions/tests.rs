use super::*;

#[test]
fn a_session_ends_after_thirty_idle_minutes() {
    let sessions = Sessions::default();
    let (token, session) = sessions.create(1, 1000);
    assert_eq!(session.user, 1);
    assert_eq!(sessions.touch(&token, 1000 + IDLE - 1).unwrap().csrf, session.csrf);
    let used_at = 1000 + IDLE - 1;
    assert!(sessions.touch(&token, used_at + IDLE - 1).is_some(), "use resets the idle clock");
    assert!(sessions.touch(&token, used_at + IDLE - 1 + IDLE).is_none());
    assert!(sessions.touch(&token, used_at).is_none(), "an expired session is gone for good");
}

#[test]
fn a_session_ends_after_twelve_hours_however_busy() {
    let sessions = Sessions::default();
    let (token, _) = sessions.create(1, 0);
    let mut now = 0;
    while now + 600 < ABSOLUTE {
        now += 600;
        assert!(sessions.touch(&token, now).is_some());
    }
    assert!(sessions.touch(&token, ABSOLUTE).is_none());
}

#[test]
fn only_the_hash_is_kept_and_logout_ends_it() {
    let sessions = Sessions::default();
    let (token, _) = sessions.create(7, 0);
    assert!(!sessions.live.lock().unwrap().contains_key(&token));
    assert!(sessions.touch("not-a-token", 0).is_none());
    assert!(sessions.has_live(7, 1));
    sessions.end(&token);
    assert!(sessions.touch(&token, 1).is_none());
    assert!(!sessions.has_live(7, 1));
    let (a, _) = sessions.create(7, 0);
    let (b, _) = sessions.create(8, 0);
    sessions.end_user(7);
    assert!(sessions.touch(&a, 1).is_none() && sessions.touch(&b, 1).is_some());
}

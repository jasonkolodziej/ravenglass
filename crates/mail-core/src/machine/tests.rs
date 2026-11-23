use super::*;


#[test]
fn sync_happy_path_reaches_idle() {
    use account_sync::*;
    let mut s = State::Disconnected;
    assert_eq!(step(&mut s, Event::Start, 0.0), vec![Effect::Connect]);
    step(&mut s, Event::Connected, 0.0);
    step(&mut s, Event::Authenticated, 0.0);
    step(&mut s, Event::FolderSynced { total: 2 }, 0.0);
    assert_eq!(s, State::Syncing { done: 1, total: 2 });
    assert_eq!(step(&mut s, Event::FolderSynced { total: 2 }, 0.0), vec![Effect::StartIdle]);
    assert_eq!(s, State::Idle);
}

#[test]
fn network_failure_backs_off_and_retries_on_its_timer_only() {
    use account_sync::*;
    let mut s = State::Disconnected;
    step(&mut s, Event::Start, 0.0);
    let fx = step(&mut s, Event::Failed(MailError::Network("reset".into())), 10.0);
    assert_eq!(fx, vec![Effect::WakeAt(12.0)]);
    assert!(step(&mut s, Event::Timer(11.0), 11.0).is_empty(), "early timer is ignored");
    assert_eq!(step(&mut s, Event::Timer(12.0), 12.0), vec![Effect::Connect]);
}

#[test]
fn auth_failure_stops_retrying() {
    use account_sync::*;
    let mut s = State::Authenticating { attempt: 0 };
    step(&mut s, Event::Failed(MailError::Auth), 0.0);
    assert_eq!(s, State::AuthFailed);
    assert!(step(&mut s, Event::Timer(1e9), 1e9).is_empty());
}

#[test]
fn undo_send_cancels_before_the_deadline() {
    use outbox::*;
    let (mut s, fx) = queue(100.0, 10.0);
    assert!(fx.contains(&Effect::WakeAt(110.0)));
    step(&mut s, Event::Undo);
    assert_eq!(s, State::Cancelled);
    assert!(step(&mut s, Event::Timer(110.0)).is_empty());
}

#[test]
fn send_after_deadline_then_retry_after_failure() {
    use outbox::*;
    let (mut s, _) = queue(0.0, 10.0);
    assert_eq!(step(&mut s, Event::Timer(10.0)), vec![Effect::Send { attempt: 1 }]);
    step(&mut s, Event::SendErr(MailError::Network("timeout".into())));
    assert_eq!(step(&mut s, Event::Retry), vec![Effect::Send { attempt: 2 }]);
    step(&mut s, Event::SendOk);
    assert_eq!(s, State::Sent);
}

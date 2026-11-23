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

#[test]
fn compose_autosaves_after_a_quiet_period_and_keeps_edits_made_while_saving() {
    use compose::*;
    let mut s = open();
    assert_eq!(step(&mut s, Event::Edit, 0.0), vec![Effect::WakeAt(AUTOSAVE_SECS)]);
    assert!(step(&mut s, Event::Timer(1.0), 1.0).is_empty(), "early timer is ignored");
    assert_eq!(step(&mut s, Event::Timer(3.0), 3.0), vec![Effect::AppendDraft]);
    step(&mut s, Event::Edit, 3.5);
    assert_eq!(step(&mut s, Event::DraftSaved, 4.0), vec![Effect::WakeAt(7.0)]);
    assert_eq!(s, State::Editing { dirty_since: Some(4.0) });
}

#[test]
fn compose_send_queues_once_and_discard_deletes_the_draft() {
    use compose::*;
    let mut s = open();
    assert_eq!(step(&mut s, Event::Send, 0.0), vec![Effect::QueueSend]);
    assert!(step(&mut s, Event::Send, 0.0).is_empty());
    let mut d = open();
    assert_eq!(step(&mut d, Event::Discard, 0.0), vec![Effect::DeleteDraft]);
    assert_eq!(d, State::Closed);
}

#[test]
fn images_stay_blocked_until_allowed_and_remember_the_sender_on_request() {
    use image_load::*;
    let (mut s, fx) = open(2, false);
    assert!(fx.is_empty());
    assert!(step(&mut s, Event::Fetched { ok: true }).is_empty(), "nothing loads while blocked");
    assert_eq!(step(&mut s, Event::Allow(Scope::Sender)), vec![Effect::RememberSender, Effect::FetchAll]);
    step(&mut s, Event::Fetched { ok: true });
    step(&mut s, Event::Fetched { ok: false });
    assert_eq!(s, State::Failed { failed: 1 });
    assert_eq!(open(3, true).1, vec![Effect::FetchAll]);
    assert_eq!(open(0, false).0, State::Loaded);
}

#[test]
fn html_view_rasters_in_tiles_and_relayouts_only_on_width() {
    use html_view::*;
    let mut s = open(680);
    assert_eq!(step(&mut s, Event::Parsed), vec![Effect::Layout { width: 680 }]);
    assert_eq!(step(&mut s, Event::LaidOut { height: 2500 }), vec![Effect::Raster { tiles: 3 }]);
    for _ in 0..3 { step(&mut s, Event::TileDone); }
    assert_eq!(s, State::Ready { width: 680, height: 2500 });
    assert!(step(&mut s, Event::Width(680)).is_empty(), "same width keeps the bitmap");
    assert_eq!(step(&mut s, Event::Repaint), vec![Effect::Raster { tiles: 3 }]);
    assert_eq!(step(&mut s, Event::Width(500)), vec![Effect::Layout { width: 500 }]);
}

#[test]
fn html_view_crash_falls_back_for_good() {
    use html_view::*;
    let mut s = open(680);
    assert_eq!(step(&mut s, Event::Crashed), vec![Effect::Fallback]);
    assert!(step(&mut s, Event::Parsed).is_empty());
    assert_eq!(s, State::Failed);
}

#[test]
fn app_locks_after_idle_time_and_unlocks_only_on_a_successful_prompt() {
    use app_lock::*;
    let policy = Policy { idle_secs: 600.0, lock_on_launch: false };
    let (mut s, fx) = launch(policy, 0.0);
    assert_eq!(fx, vec![Effect::WakeAt(600.0)]);
    step(&mut s, Event::Activity(500.0), policy);
    assert_eq!(step(&mut s, Event::Timer(600.0), policy), vec![Effect::WakeAt(1100.0)], "activity pushes the lock back");
    assert_eq!(step(&mut s, Event::Timer(1100.0), policy), vec![Effect::Conceal]);
    assert_eq!(s, State::Locked);
    assert_eq!(step(&mut s, Event::Activity(1101.0), policy), vec![Effect::Prompt]);
    assert!(step(&mut s, Event::Prompted { ok: false }, policy).is_empty());
    assert_eq!(s, State::Locked, "a failed or cancelled prompt stays locked");
    step(&mut s, Event::Unlock, policy);
    assert_eq!(step(&mut s, Event::Prompted { ok: true }, policy), vec![Effect::Reveal]);
    assert_eq!(unlocked(&mut s, policy, 1200.0), vec![Effect::WakeAt(1800.0)]);
    assert_eq!(s, State::Unlocked { last_activity: 1200.0 });
}

#[test]
fn app_lock_on_launch_conceals_first_and_never_idles_out_when_disabled() {
    use app_lock::*;
    let (s, fx) = launch(Policy { idle_secs: 0.0, lock_on_launch: true }, 0.0);
    assert_eq!((s, fx), (State::Prompting, vec![Effect::Conceal, Effect::Prompt]));
    let off = Policy { idle_secs: 0.0, lock_on_launch: false };
    let (mut s, fx) = launch(off, 0.0);
    assert!(fx.is_empty() && step(&mut s, Event::Timer(1e9), off).is_empty());
    assert_eq!(step(&mut s, Event::LockNow, off), vec![Effect::Conceal], "locking by hand always works");
    assert!(unlocked(&mut s, off, 5.0).is_empty(), "only a prompt can unlock");
    assert_eq!(s, State::Locked);
}

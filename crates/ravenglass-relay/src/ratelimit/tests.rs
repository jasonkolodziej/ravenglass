use super::*;

#[test]
fn failures_beyond_the_free_ones_back_off_exponentially() {
    let limiter = Limiter::new(3);
    for _ in 0..3 {
        assert_eq!(limiter.check("k", 100), Ok(()));
        limiter.fail("k", 100);
    }
    assert_eq!(limiter.check("k", 100), Err(1));
    assert_eq!(limiter.check("k", 101), Ok(()));
    limiter.fail("k", 101);
    assert_eq!(limiter.check("k", 101), Err(2));
    limiter.fail("k", 103);
    assert_eq!(limiter.check("k", 103), Err(4));
    assert_eq!(limiter.check("other", 103), Ok(()), "keys are independent");
}

#[test]
fn the_delay_is_capped_and_forgotten_after_an_hour() {
    let limiter = Limiter::new(0);
    for _ in 0..40 { limiter.fail("k", 0); }
    assert_eq!(limiter.check("k", 0), Err(MAX_DELAY));
    assert_eq!(limiter.check("k", FORGET), Ok(()));
    limiter.fail("k", FORGET);
    assert_eq!(limiter.check("k", FORGET), Err(2), "starts over at one failure");
}

#[test]
fn success_clears_the_key() {
    let limiter = Limiter::new(0);
    limiter.fail("k", 0);
    assert!(limiter.check("k", 0).is_err());
    limiter.succeed("k");
    assert_eq!(limiter.check("k", 0), Ok(()));
}

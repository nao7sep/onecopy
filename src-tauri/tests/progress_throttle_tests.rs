// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use std::time::{Duration, Instant};

use onecopy_lib::progress_throttle::{ProgressThrottle, INTERVAL};

#[test]
fn the_first_update_of_a_stream_is_published() {
    let mut throttle = ProgressThrottle::default();
    assert!(throttle.admit("download", false, Instant::now()));
}

#[test]
fn updates_within_the_interval_are_held_back_until_it_passes() {
    let start = Instant::now();
    let mut throttle = ProgressThrottle::default();
    assert!(throttle.admit("download", false, start));
    assert!(!throttle.admit("download", false, start + Duration::from_millis(10)));
    assert!(!throttle.admit("download", false, start + INTERVAL - Duration::from_millis(1)));
    assert!(throttle.admit("download", false, start + INTERVAL));
}

#[test]
fn a_step_change_or_completion_is_always_published() {
    let start = Instant::now();
    let mut throttle = ProgressThrottle::default();
    assert!(throttle.admit("download", false, start));
    assert!(throttle.admit("verify", false, start + Duration::from_millis(1)));
    assert!(throttle.admit("verify", true, start + Duration::from_millis(2)));
    assert!(!throttle.admit("verify", false, start + Duration::from_millis(3)));
}

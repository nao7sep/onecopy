use onecopy_lib::derived_runtime::changed_pause_classes;
use onecopy_lib::source_check_state::{Request, ResultState, SourceCheckState};

#[test]
fn bulk_pause_and_individual_resume_have_one_authority() {
    let classes = [
        "previews",
        "snapshots",
        "similarity",
        "faces",
        "video-transcripts",
        "audio-transcripts",
    ];
    let all = changed_pause_classes(0, None, true).unwrap();
    for class in classes {
        let resumed = changed_pause_classes(all, Some(class), false).unwrap();
        assert_ne!(resumed, all);
        assert_eq!(
            changed_pause_classes(resumed, Some(class), true).unwrap(),
            all
        );
        for other in classes {
            if other != class {
                assert_eq!(
                    changed_pause_classes(resumed, Some(other), true).unwrap(),
                    resumed
                );
            }
        }
    }
    assert!(changed_pause_classes(all, Some("unknown"), false).is_err());
}

#[test]
fn source_check_is_finite_and_early_resume_cannot_consume_restart_intent() {
    let mut state = SourceCheckState::default();
    assert!(state.begin(Request::Explicit, false));
    assert!(!state.begin(Request::Explicit, false));
    state.preempt();
    assert!(state.cancelled());
    assert!(!state.begin(Request::Resume, false)); // Foreground finishes before the worker retires.
    state.finish(ResultState::Stopped);
    assert!(state.waiting());
    assert!(!state.begin(Request::Resume, true)); // Another foreground waiter still has priority.
    assert!(state.begin(Request::Resume, false));
    state.finish(ResultState::Completed);
    assert!(!state.running());
    assert!(!state.begin(Request::Resume, false)); // Completion does not schedule another pass.
    assert!(state.begin(Request::Explicit, false));
}

#[test]
fn explicit_stop_wins_before_during_and_after_foreground_preemption() {
    for stop_at in 0..3 {
        let mut state = SourceCheckState::default();
        assert!(state.begin(Request::Explicit, false));
        if stop_at == 0 {
            state.stop();
        }
        state.preempt();
        if stop_at == 1 {
            state.stop();
        }
        state.finish(ResultState::Stopped);
        if stop_at == 2 {
            state.stop();
        }
        assert!(!state.running());
        assert!(!state.begin(Request::Resume, false));
    }
}

#[test]
fn source_failure_never_silently_restarts() {
    let mut state = SourceCheckState::default();
    state.begin(Request::Explicit, false);
    state.preempt();
    state.finish(ResultState::Failed);
    assert!(!state.begin(Request::Resume, false));
    assert!(matches!(state.last_result, ResultState::Failed));
}

#[test]
fn explicit_check_acknowledges_once_even_without_progress_or_changes() {
    for result in [ResultState::Completed, ResultState::CompletedWithIssues] {
        let mut state = SourceCheckState::default();
        assert!(state.begin(Request::Explicit, false));
        assert!(state.finish(result));
        assert!(!state.finish(result));
        assert!(state.begin(Request::Automatic, false));
        assert!(!state.finish(result));
    }
}

#[test]
fn preemption_keeps_explicit_feedback_but_does_not_turn_an_automatic_check_into_one() {
    for request in [Request::Explicit, Request::Automatic] {
        let mut state = SourceCheckState::default();
        assert!(state.begin(request, false));
        for _ in 0..3 {
            state.preempt();
            assert!(!state.finish(ResultState::Stopped));
            assert!(!state.begin(Request::Resume, true));
            assert!(state.begin(Request::Resume, false));
            // A refused new request cannot replace the admitted origin.
            assert!(!state.begin(Request::Explicit, false));
        }
        assert_eq!(
            state.finish(ResultState::Completed),
            request == Request::Explicit
        );
    }
}

#[test]
fn stopped_or_failed_requests_never_leak_success_feedback_to_a_later_request() {
    for result in [ResultState::Stopped, ResultState::Failed] {
        let mut state = SourceCheckState::default();
        assert!(state.begin(Request::Explicit, false));
        assert!(!state.finish(result));
        assert!(state.begin(Request::Automatic, false));
        assert!(!state.finish(ResultState::Completed));
    }
    let mut state = SourceCheckState::default();
    state.begin(Request::Explicit, false);
    state.stop();
    assert!(!state.finish(ResultState::Completed)); // Stop races a completed final step.
    state.begin(Request::Explicit, false);
    state.preempt();
    state.finish(ResultState::Stopped);
    state.stop(); // Queued continuation, with no worker left to finish it.
    assert!(!state.begin(Request::Resume, false));
    state.begin(Request::Automatic, false);
    assert!(!state.finish(ResultState::Completed));
}

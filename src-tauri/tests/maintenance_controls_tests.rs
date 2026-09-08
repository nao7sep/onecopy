use onecopy_lib::derived_runtime::changed_pause_classes;
use onecopy_lib::source_check_state::{ResultState, SourceCheckState};

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
    assert!(state.begin(true, false));
    assert!(!state.begin(true, false));
    state.preempt();
    assert!(state.cancelled());
    assert!(!state.begin(false, false)); // Foreground finishes before the worker retires.
    state.finish(ResultState::Stopped);
    assert!(state.waiting());
    assert!(!state.begin(false, true)); // Another foreground waiter still has priority.
    assert!(state.begin(false, false));
    state.finish(ResultState::Completed);
    assert!(!state.running());
    assert!(!state.begin(false, false)); // Completion does not schedule another pass.
    assert!(state.begin(true, false));
}

#[test]
fn explicit_stop_wins_before_during_and_after_foreground_preemption() {
    for stop_at in 0..3 {
        let mut state = SourceCheckState::default();
        assert!(state.begin(true, false));
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
        assert!(!state.begin(false, false));
    }
}

#[test]
fn source_failure_never_silently_restarts() {
    let mut state = SourceCheckState::default();
    state.begin(true, false);
    state.preempt();
    state.finish(ResultState::Failed);
    assert!(!state.begin(false, false));
    assert!(matches!(state.last_result, ResultState::Failed));
}

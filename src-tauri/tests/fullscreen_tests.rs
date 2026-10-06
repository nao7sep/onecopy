use onecopy_lib::fullscreen::{level, Frame, FullscreenState, Level};
use onecopy_lib::window_placement::{closing_state_for, ClosingState, NormalRectangle};

#[test]
fn a_fullscreen_window_is_raised_only_while_onecopy_is_active() {
    assert_eq!(level(true, true), Level::Raised);
    assert_eq!(level(true, false), Level::Normal);
    assert_eq!(level(false, true), Level::Normal);
    assert_eq!(level(false, false), Level::Normal);
}

#[test]
fn a_decorated_window_gets_its_exact_earlier_frame_back() {
    let mut state = FullscreenState::default();
    let earlier = Frame {
        x: 212.5,
        y: 96.0,
        width: 1280.0,
        height: 801.0,
    };
    assert!(state.enter("main", Some(earlier)));
    // A second entry while fullscreen neither re-records nor replaces it.
    assert!(!state.enter(
        "main",
        Some(Frame {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        })
    ));
    assert_eq!(state.leave("main"), Some(Some(earlier)));
    assert_eq!(state.leave("main"), None);
}

#[test]
fn main_frame_while_fullscreen_stays_out_of_its_saved_placement() {
    let mut state = FullscreenState::default();
    state.enter("main", Some(Frame {
        x: 10.0,
        y: 20.0,
        width: 800.0,
        height: 600.0,
    }));
    let whole_display = NormalRectangle {
        x: 0,
        y: 0,
        width: 3024,
        height: 1964,
    };
    // macOS reports a raised borderless window as an ordinary one.
    assert_eq!(
        closing_state_for(false, false, state.contains("main"), false, whole_display),
        ClosingState::Transient
    );
    state.leave("main");
    assert_eq!(
        closing_state_for(false, false, state.contains("main"), false, whole_display),
        ClosingState::Normal(whole_display)
    );
}

#[test]
fn windows_enter_and_leave_independently() {
    let mut state = FullscreenState::default();
    assert!(state.enter("main", None));
    assert!(state.enter("comparison-1", None));
    assert_eq!(state.leave("comparison-1"), Some(None));
    assert!(state.contains("main"));
    assert_eq!(state.labels(), vec!["main".to_string()]);
}

#[test]
fn activation_is_reported_only_when_it_changes() {
    let mut state = FullscreenState::default();
    assert_eq!(state.observe_activation(true), Some(true));
    assert_eq!(state.observe_activation(true), None);
    assert_eq!(state.observe_activation(false), Some(false));
    assert_eq!(state.observe_activation(false), None);
}

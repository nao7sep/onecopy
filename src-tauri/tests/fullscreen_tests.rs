use onecopy_lib::fullscreen::{
    landing_level, level, restored_frame, session_close_event, settle_activation, Frame, FullscreenState, Level,
    Surface,
};
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
    assert!(state.enter("main", Surface::Focused, Some(earlier)));
    // A second entry while fullscreen neither re-records nor replaces it.
    assert!(!state.enter(
        "main",
        Surface::Focused,
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
    state.enter("main", Surface::Focused, Some(Frame {
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
    // Asked to leave, the title bar comes back first and resizes the window
    // before its frame is restored: that size is still not its placement.
    state.leave("main");
    assert_eq!(
        closing_state_for(false, false, state.contains("main"), false, whole_display),
        ClosingState::Transient
    );
    state.left("main");
    assert_eq!(
        closing_state_for(false, false, state.contains("main"), false, whole_display),
        ClosingState::Normal(whole_display)
    );
}

#[test]
fn a_window_entered_again_while_leaving_stays_fullscreen_with_its_first_frame() {
    let mut state = FullscreenState::default();
    let earlier = Frame {
        x: 40.0,
        y: 60.0,
        width: 1024.0,
        height: 700.0,
    };
    state.enter("main", Surface::Focused, Some(earlier));
    assert_eq!(state.leave("main"), Some(Some(earlier)));
    assert_eq!(state.leave("main"), None);
    // Leaving windows have their normal level on the way.
    assert!(state.windows().is_empty());
    let fullscreen_frame = Frame {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };
    assert!(state.enter("main", Surface::Focused, Some(fullscreen_frame)));
    // The leave that was already queued lands, and the window stays.
    state.left("main");
    assert!(state.contains("main"));
    assert_eq!(state.windows(), vec![("main".to_string(), Surface::Focused)]);
    assert_eq!(state.leave("main"), Some(Some(earlier)));
    state.left("main");
    assert!(!state.contains("main"));
}

#[test]
fn a_destroyed_window_is_forgotten_even_while_leaving() {
    let mut state = FullscreenState::default();
    state.enter("comparison-1", Surface::Spread, None);
    state.leave("comparison-1");
    state.forget("comparison-1");
    assert!(!state.contains("comparison-1"));
}

const LAPTOP: Frame = Frame {
    x: 0.0,
    y: 0.0,
    width: 1512.0,
    height: 944.0,
};

#[test]
fn the_restored_frame_is_exact_while_its_display_remains() {
    let earlier = Frame {
        x: 1712.5,
        y: 120.0,
        width: 1280.0,
        height: 801.0,
    };
    let external = Frame {
        x: 1512.0,
        y: -200.0,
        width: 2560.0,
        height: 1415.0,
    };
    assert_eq!(restored_frame(earlier, &[LAPTOP, external], Some(LAPTOP)), earlier);
}

#[test]
fn a_frame_whose_display_was_unplugged_returns_centred_on_the_current_one() {
    let earlier = Frame {
        x: 1712.0,
        y: 120.0,
        width: 2000.0,
        height: 800.0,
    };
    assert_eq!(
        restored_frame(earlier, &[LAPTOP], Some(LAPTOP)),
        Frame {
            x: 0.0,
            y: 72.0,
            width: 1512.0,
            height: 800.0,
        }
    );
    // A sliver on a display is not enough to drag the window back.
    let sliver = Frame {
        x: LAPTOP.width - 20.0,
        ..earlier
    };
    assert_eq!(restored_frame(sliver, &[LAPTOP], Some(LAPTOP)).x, 0.0);
}

#[test]
fn windows_enter_and_leave_independently() {
    let mut state = FullscreenState::default();
    assert!(state.enter("main", Surface::Focused, None));
    assert!(state.enter("comparison-1", Surface::Spread, None));
    assert_eq!(
        state.windows(),
        vec![
            ("comparison-1".to_string(), Surface::Spread),
            ("main".to_string(), Surface::Focused),
        ]
    );
    assert_eq!(state.leave("comparison-1"), Some(None));
    assert!(state.contains("main"));
    assert_eq!(state.windows(), vec![("main".to_string(), Surface::Focused)]);
}

#[test]
fn activation_is_reported_only_when_it_changes() {
    let mut state = FullscreenState::default();
    assert_eq!(state.observe_activation(true), Some(true));
    assert_eq!(state.observe_activation(true), None);
    assert_eq!(state.observe_activation(false), Some(false));
    assert_eq!(state.observe_activation(false), None);
}

#[test]
fn a_change_landing_while_onecopy_is_inactive_is_not_raised() {
    // The change was asked for while OneCopy was active and lands after it
    // stopped being so: the level follows activation as it is then.
    assert_eq!(landing_level(true, || Ok(false)), Ok(Level::Normal));
    assert_eq!(landing_level(true, || Ok(true)), Ok(Level::Raised));
    assert_eq!(
        landing_level(false, || panic!("leaving never reads activation")),
        Ok(Level::Normal)
    );
}

#[test]
fn one_failed_window_neither_keeps_the_others_raised_nor_loses_the_activation_event() {
    let windows = vec![
        ("comparison-1".to_string(), Surface::Spread),
        ("comparison-2".to_string(), Surface::Spread),
        ("main".to_string(), Surface::Focused),
    ];
    let mut relevelled = Vec::new();
    let mut notified = None;
    let failures = settle_activation(
        Some(false),
        &windows,
        false,
        |label, _surface, level| {
            relevelled.push((label.to_string(), level));
            if label == "comparison-1" {
                Err("window is gone".to_string())
            } else {
                Ok(())
            }
        },
        |active| {
            notified = Some(active);
            Ok(())
        },
    );
    assert_eq!(
        relevelled,
        vec![
            ("comparison-1".to_string(), Level::Normal),
            ("comparison-2".to_string(), Level::Normal),
            ("main".to_string(), Level::Normal),
        ]
    );
    assert_eq!(notified, Some(false));
    assert_eq!(failures, vec!["comparison-1: window is gone".to_string()]);
}

#[test]
fn an_unchanged_activation_relevels_without_telling_main() {
    let windows = vec![("main".to_string(), Surface::Focused)];
    let mut levels = Vec::new();
    let failures = settle_activation(
        None,
        &windows,
        true,
        |_, _, level| {
            levels.push(level);
            Ok(())
        },
        |_| panic!("an unchanged activation is not sent"),
    );
    assert_eq!(levels, vec![Level::Raised]);
    assert!(failures.is_empty());
}

#[test]
fn a_system_close_of_a_session_window_goes_to_main() {
    assert_eq!(
        session_close_event("fullscreen-view"),
        Some("fullscreen-view://close-requested")
    );
    assert_eq!(
        session_close_event("comparison-1"),
        Some("comparison://close-requested")
    );
    assert_eq!(
        session_close_event("comparison-12"),
        Some("comparison://close-requested")
    );
    for label in ["main", "preview", "records", "identify-1", "comparison-", "comparison-x"] {
        assert_eq!(session_close_event(label), None, "{label}");
    }
}

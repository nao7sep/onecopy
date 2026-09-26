use super::{screen_frames_match, screen_reserves_dock_space};

#[test]
fn top_menu_bar_inset_is_not_mistaken_for_the_dock() {
    assert!(!screen_reserves_dock_space(
        (0.0, 0.0, 2560.0, 1440.0),
        (0.0, 0.0, 2560.0, 1415.0),
    ));
}

#[test]
fn left_right_and_bottom_dock_insets_are_detected() {
    let frame = (0.0, 0.0, 2560.0, 1440.0);
    assert!(screen_reserves_dock_space(
        frame,
        (80.0, 0.0, 2480.0, 1415.0)
    ));
    assert!(screen_reserves_dock_space(
        frame,
        (0.0, 0.0, 2480.0, 1415.0)
    ));
    assert!(screen_reserves_dock_space(
        frame,
        (0.0, 80.0, 2560.0, 1335.0)
    ));
}

#[test]
fn a_fullscreen_window_matches_the_captured_dock_display_by_frame() {
    assert!(screen_frames_match(
        (0.0, 0.0, 2560.0, 1440.0),
        (0.5, -0.5, 2560.0, 1440.0),
    ));
    assert!(!screen_frames_match(
        (0.0, 0.0, 2560.0, 1440.0),
        (2560.0, 0.0, 2560.0, 1440.0),
    ));
}

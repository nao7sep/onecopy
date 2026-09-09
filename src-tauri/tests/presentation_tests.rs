use onecopy_lib::presentation_runtime::PresentationState;

#[test]
fn activation_changes_chrome_without_changing_window_membership() {
    let mut state = PresentationState::default();
    state.set_fullscreen("preview", true);
    assert!(state.hides_system_chrome(true));
    assert!(!state.hides_system_chrome(false));
    assert!(state.contains("preview"));
    assert!(state.hides_system_chrome(true));
}

#[test]
fn closing_one_fullscreen_window_does_not_dismantle_another() {
    let mut state = PresentationState::default();
    state.set_fullscreen("preview", true);
    state.set_fullscreen("viewer", true);
    state.set_fullscreen("viewer", false);
    assert!(state.contains("preview"));
    assert!(!state.contains("viewer"));
    assert!(state.hides_system_chrome(true));
    state.set_fullscreen("preview", false);
    assert!(!state.hides_system_chrome(true));
}

#[test]
fn repeated_entry_and_unrelated_exit_do_not_change_membership() {
    let mut state = PresentationState::default();
    state.set_fullscreen("preview", true);
    state.set_fullscreen("preview", true);
    state.set_fullscreen("comparison-1", false);
    assert!(state.contains("preview"));
    state.set_fullscreen("preview", false);
    assert!(!state.hides_system_chrome(true));
}

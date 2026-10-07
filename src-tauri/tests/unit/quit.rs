use super::session_quit;

#[test]
fn only_a_quit_apple_event_with_a_session_reason_is_os_takeover() {
    let quit = Some(u32::from_be_bytes(*b"quit"));
    assert!(!session_quit(quit, false), "Dock quit is ordinary user quit");
    assert!(session_quit(quit, true));
    assert!(!session_quit(None, true));
    assert!(!session_quit(Some(u32::from_be_bytes(*b"open")), true));
}

#[test]
fn an_acknowledgement_from_a_cancelled_session_cannot_release_a_later_save() {
    let id = {
        let mut saved = super::SAVED.0.lock().unwrap();
        saved.0 += 1;
        saved.1 = false;
        saved.0
    };
    super::session_end_saved(id - 1);
    assert!(!super::SAVED.0.lock().unwrap().1);
    super::session_end_saved(id);
    assert!(super::SAVED.0.lock().unwrap().1);
}

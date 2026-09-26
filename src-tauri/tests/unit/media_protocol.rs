use super::*;

#[test]
fn a_panicking_handler_still_answers_its_request() {
    let response = answer(|| panic!("handler failed"));
    assert_eq!(response.status(), tauri::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(answer(not_found).status(), tauri::http::StatusCode::NOT_FOUND);
}

#[test]
fn only_content_addressed_entries_are_immutable() {
    assert_eq!(cache_control("3f2a9c"), "public, max-age=31536000, immutable");
    // A provisional key's entry changes with its path's file, and a rebuild
    // hands the key to another file.
    assert_eq!(cache_control("p17"), "no-store");
    assert_eq!(cache_control("p17-3"), "no-store");
}

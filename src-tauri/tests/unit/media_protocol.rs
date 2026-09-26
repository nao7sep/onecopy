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

#[test]
fn valid_key_accepts_ordinary_media_keys_and_rejects_traversal() {
    assert!(valid_key("thumb-3f2a9c"));
    assert!(valid_key("strip-3f2a9c-4"));
    assert!(valid_key("p17"));
    assert!(!valid_key(""));
    // A crafted key climbing out of its shard directory (R6-08): `hash` here
    // is "../x", and CachePaths shards on its first two characters, so an
    // unvalidated key would resolve to `<cache root>/thumbs/../x.webp`.
    assert!(!valid_key("thumb-../x"));
    assert!(!valid_key("thumb-..%2fx"));
    assert!(!valid_key("thumb-a/b"));
    assert!(!valid_key("thumb-a.b"));
}

#[test]
fn mediacache_refuses_a_key_that_would_escape_the_cache_root() {
    let request = tauri::http::Request::builder()
        .uri("mediacache://localhost/thumb-../x")
        .body(Vec::new())
        .unwrap();
    assert_eq!(serve_cache(&request).status(), tauri::http::StatusCode::NOT_FOUND);
}

#[test]
fn mediafile_refuses_a_key_that_would_escape_the_index_lookup() {
    let request = tauri::http::Request::builder()
        .uri("mediafile://localhost/../etc-passwd")
        .body(Vec::new())
        .unwrap();
    assert_eq!(serve_original(&request).status(), tauri::http::StatusCode::NOT_FOUND);
}

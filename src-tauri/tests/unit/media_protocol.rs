use super::*;

#[test]
fn a_panicking_handler_still_answers_its_request() {
    let response = answer(|| panic!("handler failed"));
    assert_eq!(response.status(), tauri::http::StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(answer(not_found).status(), tauri::http::StatusCode::NOT_FOUND);
}

#[test]
fn only_content_addressed_entries_are_immutable() {
    let directives = |key: &str| -> Vec<String> {
        cache_control(key).split(',').map(|directive| directive.trim().to_ascii_lowercase()).collect()
    };
    let content = directives("3f2a9c");
    assert!(content.iter().any(|directive| directive == "immutable"), "{content:?}");
    assert!(
        content.iter().any(|directive| directive
            .strip_prefix("max-age=")
            .and_then(|seconds| seconds.parse::<u64>().ok())
            .is_some_and(|seconds| seconds > 0)),
        "{content:?}"
    );
    assert!(!content.iter().any(|directive| directive == "no-store"), "{content:?}");
    // A provisional key's entry changes with its path's file, and a rebuild
    // hands the key to another file.
    for key in ["p17", "p17-3"] {
        let provisional = directives(key);
        assert!(provisional.iter().any(|directive| directive == "no-store"), "{key}: {provisional:?}");
        assert!(!provisional.iter().any(|directive| directive == "immutable"), "{key}: {provisional:?}");
    }
}

#[test]
fn valid_key_accepts_ordinary_media_keys_and_rejects_traversal() {
    assert!(valid_key("thumb-3f2a9c"));
    assert!(valid_key("strip-3f2a9c-4"));
    assert!(valid_key("p17"));
    assert!(!valid_key(""));
    // A crafted key climbing out of its shard directory: `hash` here
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

// A drive that stops answering answers 504 for the request that gave up on
// it and 503 while it has not answered, never a hang and never 404.
#[test]
fn an_original_on_a_drive_that_does_not_answer_is_504_then_503() {
    use crate::volume_io::{FakeStallingVolume, Op};
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("clip.mp4");
    std::fs::write(&original, vec![1u8; 4096]).unwrap();
    let request = tauri::http::Request::builder()
        .uri("mediafile://localhost/p1")
        .header("Range", "bytes=0-99")
        .body(Vec::new())
        .unwrap();
    assert_eq!(serve_path(&request, &original).status(), 206);

    let volume = FakeStallingVolume::mount(dir.path(), std::time::Duration::from_millis(200));
    volume.stall(&[Op::Open], None);
    let started = std::time::Instant::now();
    assert_eq!(serve_path(&request, &original).status(), 504);
    assert_eq!(serve_path(&request, &original).status(), 503);
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    volume.release();
    assert!(volume.wait_until_settled(std::time::Duration::from_secs(5)));
    assert_eq!(serve_path(&request, &original).status(), 206);
}

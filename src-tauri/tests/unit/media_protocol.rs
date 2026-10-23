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

#[test]
fn capacity_refusal_is_bounded_retryable_and_never_image_success() {
    let response = response_capacity_unavailable("Response allocation failed");
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["Retry-After"], "30");
    assert_eq!(response.headers()["Content-Type"], "text/plain; charset=utf-8");
    assert_eq!(response.body(), b"Response allocation failed");
}

#[test]
fn response_reads_are_chunked_and_never_grow_the_admitted_body() {
    struct Reader {
        remaining: usize,
        largest_read: usize,
    }
    impl std::io::Read for Reader {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.largest_read = self.largest_read.max(bytes.len());
            let read = bytes.len().min(self.remaining);
            bytes[..read].fill(7);
            self.remaining -= read;
            Ok(read)
        }
    }
    let mut reader = Reader { remaining: 4 * 1024 * 1024, largest_read: 0 };
    let mut body = vec![0; 3 * 1024 * 1024 + 1];
    read_response_body(&mut reader, &mut body).unwrap();
    assert_eq!(reader.largest_read, 1024 * 1024);
    assert_eq!(reader.remaining, 1024 * 1024 - 1);
    assert!(body.iter().all(|byte| *byte == 7));
    let mut truncated = std::io::Cursor::new(b"short");
    assert_eq!(read_response_body(&mut truncated, &mut body).unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn cached_images_preserve_content_headers_and_lease_through_response_parts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("fullres.png");
    let bytes = b"\x89PNG\r\n\x1a\nimage bytes";
    std::fs::write(&file, bytes).unwrap();
    let response = serve_cache_path(&file, "3f2a9c");
    assert_eq!(response.status(), 200);
    assert_eq!(response.body(), bytes);
    assert_eq!(response.headers()["Content-Type"], "image/png");
    assert_eq!(response.headers()["Cache-Control"], cache_control("3f2a9c"));
    let (parts, body) = response.into_parts();
    assert_eq!(body, bytes);
    assert!(parts.extensions.get::<std::sync::Arc<crate::resource_limits::ResponseBytesLease>>().is_some());
    assert_eq!(serve_cache_path(&file, "p17").headers()["Cache-Control"], "no-store");
}

#[cfg(all(unix, target_os = "macos"))]
#[test]
#[ignore = "requires a filesystem supporting non-UTF-8 filenames; APFS rejects these names"]
fn original_reads_the_native_path_without_lossy_reopening() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(std::ffi::OsString::from_vec(b"image-\xff.png".to_vec()));
    std::fs::write(&file, b"original image").unwrap();
    let request = tauri::http::Request::builder().body(Vec::new()).unwrap();
    let response = serve_path(&request, &file);
    assert_eq!(response.status(), 200);
    assert_eq!(response.body(), b"original image");
    assert_eq!(response.headers()["Content-Type"], "image/png");
    assert!(response.extensions().get::<std::sync::Arc<crate::resource_limits::ResponseBytesLease>>().is_some());
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

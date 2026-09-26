use super::*;

#[test]
fn unicode_markers_are_exact_including_utf32() {
    assert_eq!(
        decode_automatic(b"\xEF\xBB\xBFhello", "utf-8")
            .unwrap()
            .unwrap()
            .0,
        "hello"
    );
    let utf32 = [0xFF, 0xFE, 0, 0, b'A', 0, 0, 0];
    assert_eq!(decode_automatic(&utf32, "utf-8").unwrap().unwrap().0, "A");
}

#[test]
fn exact_utf8_wins_before_detection() {
    let decoded = decode_automatic("日本語".as_bytes(), "windows-1252")
        .unwrap()
        .unwrap();
    assert_eq!(decoded, ("日本語".to_string(), "utf-8".to_string()));
}

#[test]
fn binary_bytes_do_not_fall_through_to_a_legacy_decoder() {
    assert!(decode_automatic(b"GIF89a\0\x01\x02", "windows-1252")
        .unwrap()
        .is_none());
}

#[test]
fn explicit_encoding_may_override_automatic_binary_guard() {
    assert_eq!(decode_named(&[b'A', 0, b'B', 0], "utf-16le").unwrap(), "AB");
}

#[test]
fn whole_file_cap_returns_attributes_without_reading_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.txt");
    std::fs::write(&path, b"too long").unwrap();
    assert!(matches!(
        preview_file(&path, 3, "utf-8", None).unwrap(),
        PreviewBody::Attributes { byte_size: 8, .. }
    ));
}

// (C-L2) The read itself is bounded, not just the pre-check: a boundary-fit
// file still decodes, and an unclamped setting can no longer overflow the
// `max_bytes + 1` read bound.
#[test]
fn a_file_exactly_at_the_gate_still_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exact.txt");
    std::fs::write(&path, b"hello").unwrap();
    assert!(matches!(
        preview_file(&path, 5, "utf-8", None).unwrap(),
        PreviewBody::Text { byte_size: 5, .. }
    ));
}

#[test]
fn an_unbounded_setting_is_clamped_instead_of_overflowing_the_read_bound() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.txt");
    std::fs::write(&path, b"hello").unwrap();
    // Without clamping to MAX_ALLOWED_BYTES first, `max_bytes + 1` on
    // `u64::MAX` overflows (panics in debug) before the file is ever read.
    assert!(matches!(
        preview_file(&path, u64::MAX, "utf-8", None).unwrap(),
        PreviewBody::Text { byte_size: 5, .. }
    ));
}

#[test]
fn every_presented_encoding_is_a_working_canonical_decoder() {
    for label in encodings() {
        assert_eq!(canonical_label(label).unwrap(), *label, "{label}");
        assert!(decode_named(b"", label).is_ok(), "{label}");
    }
}

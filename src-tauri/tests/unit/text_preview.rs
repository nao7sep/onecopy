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

// content-presentation.md D8: automatic decoding reports which step produced
// its result, so the picker can show a marker guess, an exact match, and a
// detector guess as the different confidences they are. (The fallback step
// is exercised by `binary_bytes_do_not_fall_through_to_a_legacy_decoder`'s
// sibling paths in practice, but is not independently reachable by a crafted
// byte string here: `chardetng`'s single-byte candidates accept almost any
// input without error, so the detector step wins first.)
#[test]
fn automatic_decoding_reports_which_step_produced_the_result() {
    assert_eq!(
        decode_automatic(b"\xEF\xBB\xBFhello", "utf-8")
            .unwrap()
            .unwrap()
            .2,
        Some("marker")
    );
    assert_eq!(
        decode_automatic("plain ascii".as_bytes(), "utf-8")
            .unwrap()
            .unwrap()
            .2,
        Some("exact")
    );
    assert_eq!(
        decode_automatic(&[224, 128, 128], "utf-8").unwrap().unwrap().2,
        Some("detected")
    );
}

#[test]
fn exact_utf8_wins_before_detection() {
    let decoded = decode_automatic("日本語".as_bytes(), "windows-1252")
        .unwrap()
        .unwrap();
    assert_eq!(
        decoded,
        ("日本語".to_string(), "utf-8".to_string(), Some("exact"))
    );
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

// content-presentation.md D7: invalid bytes under a selected/fallback
// encoding render replacement characters instead of failing the whole file.
// `decode_named` is the exact function `decode_automatic`'s configured-
// fallback step calls, so this covers both the manual-pick and fallback
// routes at once; only the automatic detector's OWN guess stays strict
// (unchanged, and covered by `binary_bytes_do_not_fall_through_...`).
#[test]
fn an_explicit_or_fallback_choice_replaces_invalid_bytes_instead_of_failing() {
    // 0xFF is not valid UTF-8 on its own.
    let text = decode_named(b"before\xFFafter", "utf-8").unwrap();
    assert_eq!(text, "before\u{FFFD}after");
}

#[test]
fn invalid_utf32_code_points_become_replacement_characters() {
    // 0x00110000 exceeds the Unicode range and is not a valid `char`.
    let out_of_range = [0x00, 0x11, 0x00, 0x00];
    assert_eq!(decode_utf32(&out_of_range, false).unwrap(), "\u{FFFD}");
    // A byte count that is not a multiple of four remains a genuine
    // structural failure — there is no four-byte unit to substitute for.
    assert!(decode_utf32(&out_of_range[..3], false).is_err());
}

#[test]
fn every_presented_encoding_is_a_working_canonical_decoder() {
    for label in encodings() {
        assert_eq!(canonical_label(label).unwrap(), *label, "{label}");
        assert!(decode_named(b"", label).is_ok(), "{label}");
    }
}

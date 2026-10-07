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
// detector guess as the different confidences they are.
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

// content-presentation.md D8 / R5.3 untested contract: the FALLBACK step,
// reached only when the detector's own guess errors or is not convincingly
// textual. These lead-byte-then-space Shift_JIS fragments make chardetng
// guess an encoding whose decode has errors, so `decode_automatic` falls
// through to the CONFIGURED fallback and decodes losslessly with U+FFFD in
// place of the bytes that do not fit (content-presentation.md D7) rather
// than failing the whole file.
#[test]
fn falls_through_to_the_configured_fallback_when_detection_itself_errors() {
    let bytes = [0x81, 0x20, 0x82, 0x20];
    let decoded = decode_automatic(&bytes, "shift_jis").unwrap().unwrap();
    assert_eq!(decoded.1, "shift_jis");
    assert_eq!(decoded.2, Some("fallback"));
    assert!(decoded.0.contains('\u{FFFD}'));

    // The step follows whichever encoding is CONFIGURED, not a fixed one.
    let decoded_utf16 = decode_automatic(&bytes, "utf-16le").unwrap().unwrap();
    assert_eq!(decoded_utf16.1, "utf-16le");
    assert_eq!(decoded_utf16.2, Some("fallback"));
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
    for label in options().encodings {
        assert_eq!(canonical_label(label).unwrap(), *label, "{label}");
        assert!(decode_named(b"", label).is_ok(), "{label}");
    }
}

#[test]
fn text_limit_is_internal_while_encoding_remains_configurable() {
    let defaults = Limits::from_config(None);
    assert_eq!(defaults.max_bytes, DEFAULT_MAX_BYTES);
    assert_eq!(defaults.fallback_encoding, DEFAULT_FALLBACK_ENCODING);
    let saved = Limits::from_config(Some(&serde_json::json!({
        "textPreviewMaxBytes": MAX_ALLOWED_BYTES * 2,
        "textFallbackEncoding": "shift_jis",
    })));
    assert_eq!(saved.max_bytes, DEFAULT_MAX_BYTES);
    assert_eq!(saved.fallback_encoding, "shift_jis");
    assert_eq!(
        Limits::from_config(Some(&serde_json::json!({ "textPreviewMaxBytes": 0 }))).max_bytes,
        DEFAULT_MAX_BYTES
    );
}

// A text file on a drive that does not answer shows that condition within
// the read bound instead of a failed read or an endless wait.
#[test]
fn a_file_on_a_drive_that_does_not_answer_shows_as_not_responding() {
    use crate::volume_io::{FakeStallingVolume, Op};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, b"hello").unwrap();
    let volume = FakeStallingVolume::mount(dir.path(), std::time::Duration::from_millis(200));
    volume.stall(&[Op::Read], None);
    let started = std::time::Instant::now();
    match preview_file(&path, 1024, "utf-8", None).unwrap() {
        PreviewBody::Attributes { reason_code, .. } => {
            assert_eq!(reason_code, Some("preview-not-responding"))
        }
        _ => panic!("expected the not-responding attributes body"),
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    volume.release();
    assert!(volume.wait_until_settled(std::time::Duration::from_secs(5)));
    assert!(matches!(
        preview_file(&path, 1024, "utf-8", None).unwrap(),
        PreviewBody::Text { .. }
    ));
}

// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::live_photo::*;

// Builders for synthetic box trees, so the parser is tested against the
// documented grammar rather than only itself.
fn boxed(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&(8 + payload.len() as u32).to_be_bytes());
    out.extend_from_slice(box_type);
    out.extend_from_slice(payload);
    out
}

fn keys_box(names: &[&str]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&0u32.to_be_bytes()); // version/flags
    payload.extend_from_slice(&(names.len() as u32).to_be_bytes());
    for name in names {
        payload.extend_from_slice(&(8 + name.len() as u32).to_be_bytes());
        payload.extend_from_slice(b"mdta");
        payload.extend_from_slice(name.as_bytes());
    }
    boxed(b"keys", &payload)
}

fn ilst_entry(index: u32, value: &str) -> Vec<u8> {
    let mut data_payload = Vec::new();
    data_payload.extend_from_slice(&1u32.to_be_bytes()); // type: UTF-8
    data_payload.extend_from_slice(&0u32.to_be_bytes()); // locale
    data_payload.extend_from_slice(value.as_bytes());
    let data = boxed(b"data", &data_payload);
    let mut out = Vec::new();
    out.extend_from_slice(&(8 + data.len() as u32).to_be_bytes());
    out.extend_from_slice(&index.to_be_bytes());
    out.extend_from_slice(&data);
    out
}

fn moov_with(keys: &[&str], entries: &[(u32, &str)], iso_fullbox_meta: bool) -> Vec<u8> {
    let mut ilst_payload = Vec::new();
    for (index, value) in entries {
        ilst_payload.extend_from_slice(&ilst_entry(*index, value));
    }
    let ilst = boxed(b"ilst", &ilst_payload);
    let keys = keys_box(keys);
    let mut meta_payload = Vec::new();
    if iso_fullbox_meta {
        meta_payload.extend_from_slice(&0u32.to_be_bytes()); // version/flags
    }
    meta_payload.extend_from_slice(&keys);
    meta_payload.extend_from_slice(&ilst);
    boxed(b"meta", &meta_payload)
}

#[test]
fn extracts_the_identifier_from_a_quicktime_style_meta() {
    let moov = moov_with(
        &["com.apple.quicktime.creationdate", CONTENT_IDENTIFIER_KEY],
        &[(1, "2026-08-08T12:00:00+0900"), (2, "8FDD1AD2-2D2C-4E4C-99E8")],
        false,
    );
    assert_eq!(
        moov_content_identifier(&moov).as_deref(),
        Some("8FDD1AD2-2D2C-4E4C-99E8")
    );
}

#[test]
fn extracts_from_an_iso_fullbox_meta() {
    let moov = moov_with(&[CONTENT_IDENTIFIER_KEY], &[(1, "UUID-1234")], true);
    assert_eq!(moov_content_identifier(&moov).as_deref(), Some("UUID-1234"));
}

#[test]
fn absent_key_or_value_yields_none() {
    let moov = moov_with(&["com.apple.quicktime.creationdate"], &[(1, "x")], false);
    assert_eq!(moov_content_identifier(&moov), None);
    let empty = moov_with(&[CONTENT_IDENTIFIER_KEY], &[], false);
    assert_eq!(moov_content_identifier(&empty), None);
}

#[test]
fn malformed_boxes_never_panic() {
    assert_eq!(moov_content_identifier(&[]), None);
    assert_eq!(moov_content_identifier(&[0, 0, 0, 3, b'x']), None);
    let mut truncated = moov_with(&[CONTENT_IDENTIFIER_KEY], &[(1, "u")], false);
    truncated.truncate(truncated.len() / 2);
    let _ = moov_content_identifier(&truncated); // no panic is the assertion
}

// Round-trip against a REAL file: ffmpeg (the managed install) writes the
// mdta key with -movflags use_metadata_tags; the parser must read it back.
// Run with `cargo test live_content_identifier -- --ignored --nocapture`.
#[test]
#[ignore]
#[serial_test::serial(backup_store)]
fn live_content_identifier_round_trip() {
    use onecopy_lib::binaries_manager;
    let dir = tempfile::Builder::new()
        .prefix("onecopy-livephoto-")
        .tempdir()
        .unwrap();
    binaries_manager::install_or_update(dir.path(), |p, d| eprintln!("[{p}] {d}"))
        .expect("ffmpeg install");
    let ffmpeg = binaries_manager::ffmpeg_path(dir.path());

    let clip = dir.path().join("live.mov");
    let status = std::process::Command::new(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=2:size=320x240:rate=12")
        .args([
            "-metadata",
            &format!("{CONTENT_IDENTIFIER_KEY}=TEST-UUID-0001"),
            "-movflags",
            "use_metadata_tags",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(status.success(), "clip synthesis");

    assert_eq!(
        quicktime_content_identifier(&clip).as_deref(),
        Some("TEST-UUID-0001"),
        "the identifier written by ffmpeg must round-trip through the parser"
    );
}

// Tests for the pure media-protocol logic, exercising the crate's public
// API from outside shipped source (tests-folder conventions, Rust form).

use onecopy_lib::media_protocol::*;

#[test]
fn byte_ranges_cover_the_forms_and_the_edges() {
    // Explicit range, clamped end, open end, suffix.
    assert_eq!(parse_byte_range("bytes=0-99", 1000), Some((0, 99)));
    assert_eq!(parse_byte_range("bytes=0-9999", 1000), Some((0, 999)));
    assert_eq!(parse_byte_range("bytes=500-", 1000), Some((500, 999)));
    assert_eq!(parse_byte_range("bytes=-100", 1000), Some((900, 999)));
    // Suffix larger than the file clamps to the whole file.
    assert_eq!(parse_byte_range("bytes=-5000", 1000), Some((0, 999)));
    // Rejections: start past EOF, inverted, zero-length file, zero suffix,
    // and garbage.
    assert_eq!(parse_byte_range("bytes=1000-", 1000), None);
    assert_eq!(parse_byte_range("bytes=9-5", 1000), None);
    assert_eq!(parse_byte_range("bytes=0-", 0), None);
    assert_eq!(parse_byte_range("bytes=-0", 1000), None);
    assert_eq!(parse_byte_range("items=0-5", 1000), None);
    assert_eq!(parse_byte_range("bytes=abc-def", 1000), None);
    // Multi-range: only the first is honored (single-range protocol).
    assert_eq!(parse_byte_range("bytes=0-1,5-9", 1000), Some((0, 1)));
    // A single byte at each edge.
    assert_eq!(parse_byte_range("bytes=0-0", 1000), Some((0, 0)));
    assert_eq!(parse_byte_range("bytes=999-999", 1000), Some((999, 999)));
}

#[test]
fn content_types_map_by_extension_case_insensitively() {
    assert_eq!(content_type_for("/a/B.JPG"), "image/jpeg");
    assert_eq!(content_type_for("/a/clip.MOV"), "video/quicktime");
    assert_eq!(content_type_for("/a/file.xyz"), "application/octet-stream");
}

#[test]
fn sniffing_reads_magic_bytes_not_names() {
    assert_eq!(sniff_image_content_type(b"\x89PNG\r\n"), "image/png");
    assert_eq!(sniff_image_content_type(&[0xFF, 0xD8, 0xFF]), "image/jpeg");
    assert_eq!(sniff_image_content_type(b"GIF89a"), "image/gif");
    assert_eq!(sniff_image_content_type(b"RIFF....WEBP"), "image/webp");
    assert_eq!(sniff_image_content_type(b""), "image/webp");
}

// The span decision: what the handler actually reads into memory.

const GB: u64 = 1024 * 1024 * 1024;

#[test]
fn an_open_ended_range_is_capped_to_a_bounded_span() {
    // Chromium sends exactly this for media. Uncapped it resolves to the whole
    // file, which the synchronous handler would read on the main thread.
    let (start, end, status) = resolve_range(Some("bytes=0-"), 4 * GB, true);
    assert_eq!(start, 0);
    assert_eq!(status, 206);
    assert_eq!(end - start + 1, MAX_SPAN, "must serve one bounded span");
}

#[test]
fn an_explicit_small_range_is_honored_exactly() {
    let (start, end, status) = resolve_range(Some("bytes=1000-2000"), 4 * GB, true);
    assert_eq!((start, end, status), (1000, 2000, 206));
}

#[test]
fn a_large_explicit_range_is_capped_from_its_own_start() {
    let (start, end, _) = resolve_range(Some("bytes=1048576-"), 4 * GB, true);
    assert_eq!(start, 1048576);
    assert_eq!(end - start + 1, MAX_SPAN);
}

#[test]
fn a_rangeless_video_gets_the_head_chunk() {
    let (start, end, status) = resolve_range(None, 40 * 1024 * 1024, true);
    assert_eq!((start, status), (0, 206));
    assert_eq!(end + 1, HEAD_CHUNK);
}

#[test]
fn a_rangeless_image_is_served_whole_however_big() {
    // The 100% view loads the ORIGINAL through this protocol and sends no
    // Range. A truncated 206 renders as a broken image, not a partial one.
    let total = 40 * 1024 * 1024;
    let (start, end, status) = resolve_range(None, total, false);
    assert_eq!((start, end, status), (0, total - 1, 200));
}

#[test]
fn a_small_rangeless_file_is_served_whole() {
    let (start, end, status) = resolve_range(None, 1234, true);
    assert_eq!((start, end, status), (0, 1233, 200));
}

#[test]
fn an_empty_file_does_not_underflow() {
    let (start, end, status) = resolve_range(None, 0, true);
    assert_eq!((start, end, status), (0, 0, 200));
    assert_eq!(response_length(0, start, end), 0, "the handler reads no byte");
}

#[test]
fn images_are_not_streamable_and_video_is() {
    assert!(is_streamable("video/mp4"));
    assert!(!is_streamable("image/jpeg"));
    assert!(!is_streamable("application/octet-stream"));
}

#[test]
fn audio_types_resolve_and_stream() {
    // Audio other-files play in the preview (decided 2026-08-16): the type
    // map must name them (a generic octet-stream makes the webview download
    // instead of play) and the streamable gate must include them so the
    // head-chunk shortcut applies.
    for (name, expected) in [
        ("memo.m4a", "audio/mp4"),
        ("song.mp3", "audio/mpeg"),
        ("raw.wav", "audio/wav"),
        ("lossless.flac", "audio/flac"),
        ("talk.opus", "audio/opus"),
    ] {
        let content_type = content_type_for(name);
        assert_eq!(content_type, expected, "{name}");
        assert!(is_streamable(content_type), "{name} must stream");
    }
}

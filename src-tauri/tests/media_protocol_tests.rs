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

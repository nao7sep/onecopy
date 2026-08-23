//! The pure half of the media-serving protocols: HTTP Range parsing, the
//! extension→content-type map, and magic-byte sniffing for cache entries.
//! Extracted from the protocol handlers in lib.rs (which is the Tauri
//! bootstrap and carries no test module) so the fiddly decisions are unit
//! tested — a Range off-by-one surfaces as a video that mis-seeks, which is
//! miserable to attribute from the player's behavior.

use crate::extensions;

/// `bytes=start-end` / `bytes=start-` / `bytes=-suffix`, single range only.
pub fn parse_byte_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (start_text, end_text) = spec.split_once('-')?;
    if start_text.is_empty() {
        // Suffix form: the last N bytes.
        let suffix: u64 = end_text.parse().ok()?;
        if suffix == 0 || total == 0 {
            return None;
        }
        return Some((total.saturating_sub(suffix), total - 1));
    }
    let start: u64 = start_text.parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if end_text.is_empty() {
        total - 1
    } else {
        end_text.parse::<u64>().ok()?.min(total - 1)
    };
    (start <= end).then_some((start, end))
}

pub fn content_type_for(path: &str) -> &'static str {
    match extensions::lowercase_ext(path).as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" | "heif" | "hif" => "image/heif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "mpg" | "mpeg" => "video/mpeg",
        "wmv" => "video/x-ms-wmv",
        "3gp" => "video/3gpp",
        "mts" | "m2ts" => "video/mp2t",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "amr" => "audio/amr",
        _ => "application/octet-stream",
    }
}

/// Magic-byte sniff over the formats the cache can hold (preview entries may
/// be byte-copies of originals under the `.webp` cache name); WebP and
/// anything unrecognized report as WebP, the tree's native encode format.
pub fn sniff_image_content_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else {
        "image/webp"
    }
}

/// The largest span one response will carry. A Chromium webview opens every
/// media resource with a literal `Range: bytes=0-`, which resolves — correctly
/// per spec — to the entire file, so without a cap the handler allocates and
/// reads the whole thing. That matters more here than it looks: the mediafile
/// protocol is registered SYNCHRONOUSLY, so wry runs it inline on the main
/// thread and a multi-gigabyte video freezes the whole app rather than just
/// the player. Returning fewer bytes than asked for is legal for a 206 as long
/// as Content-Range describes what was actually sent; the player simply asks
/// for the next span.
pub const MAX_SPAN: u64 = 8 * 1024 * 1024;

/// Served to a streamable resource that asked for no range at all, so the
/// player switches to ranged loading instead of pulling the file whole.
pub const HEAD_CHUNK: u64 = 1024 * 1024;

/// Above this, a rangeless streamable request gets `HEAD_CHUNK` rather than
/// the whole file.
pub const WHOLE_FILE_LIMIT: u64 = 32 * 1024 * 1024;

/// True when a resource can be delivered progressively. Images cannot: a
/// truncated JPEG is a broken tile, not a partial one, so the head-chunk
/// shortcut must never apply to them.
pub fn is_streamable(content_type: &str) -> bool {
    content_type.starts_with("video/") || content_type.starts_with("audio/")
}

/// The byte span to serve and the status to serve it with.
///
/// Returns `(start, end_inclusive, status)`. `total == 0` yields an empty
/// 200 rather than an underflowed span.
pub fn resolve_range(
    range_header: Option<&str>,
    total: u64,
    streamable: bool,
) -> (u64, u64, u16) {
    if total == 0 {
        return (0, 0, 200);
    }
    match range_header.and_then(|header| parse_byte_range(header, total)) {
        Some((start, end)) => {
            let capped = end.min(start.saturating_add(MAX_SPAN - 1)).min(total - 1);
            (start, capped, 206)
        }
        // Only streamable resources get the head-chunk treatment; an image
        // asked for without a Range must arrive whole or it cannot render.
        None if streamable && total > WHOLE_FILE_LIMIT => {
            (0, HEAD_CHUNK.min(total) - 1, 206)
        }
        None => (0, total - 1, 200),
    }
}

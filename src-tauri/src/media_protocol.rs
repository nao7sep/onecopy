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

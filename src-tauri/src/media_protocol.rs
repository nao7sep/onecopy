//! The media-serving protocol authority: indexed-original and derived-cache
//! I/O plus the pure Range, content-type, and magic-byte policy they share.
//! The Tauri bootstrap only registers these handlers.

use std::io::{Read, Seek, SeekFrom};

use rusqlite::OptionalExtension;
use serde_json::json;

use crate::extensions;

fn not_found() -> tauri::http::Response<Vec<u8>> {
    let mut response = tauri::http::Response::new(Vec::new());
    *response.status_mut() = tauri::http::StatusCode::NOT_FOUND;
    response
}

/// Serves an original by indexed content hash or `path-<id>`. The webview
/// never receives a filesystem path; large streamable files are range-capped
/// so this synchronous protocol cannot read them wholesale on the UI thread.
pub(crate) fn serve_original(
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let warn_404 = |reason: &str, detail: String| {
        crate::logging::warn(
            "mediafile request failed",
            json!({ "reason": reason, "detail": detail }),
        );
        not_found()
    };
    let Some(data_root) = crate::DATA_ROOT.get() else {
        return warn_404("data root unset", String::new());
    };
    let key = request.uri().path().trim_start_matches('/');
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
        return not_found();
    }

    let conn = match crate::index_store::open(
        &data_root.join(crate::storage::INDEX_DB_FILE_NAME),
    ) {
        Ok(conn) => conn,
        Err(error) => return warn_404("index open failed", error),
    };
    let path: rusqlite::Result<Option<String>> = match key.strip_prefix("path-") {
        Some(id) => match id.parse::<i64>() {
            Ok(id) => conn
                .query_row(
                "SELECT abs_path FROM paths WHERE id = ?1 AND missing = 0",
                [id],
                |row| row.get::<_, String>(0),
            )
                .optional(),
            Err(_) => return not_found(),
        },
        None => conn
            .query_row(
                "SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional(),
    };
    let path = match path {
        Ok(path) => path,
        Err(error) => return warn_404("path lookup failed", error.to_string()),
    };
    let Some(path) = path else {
        return warn_404("no live path for key", key.to_string());
    };

    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) => return warn_404("original unreadable", format!("{path}: {error}")),
    };
    let total = match file.metadata().map(|metadata| metadata.len()) {
        Ok(total) => total,
        Err(error) => return warn_404("metadata failed", format!("{path}: {error}")),
    };
    let content_type = content_type_for(&path);
    let range_header = request
        .headers()
        .get("Range")
        .and_then(|value| value.to_str().ok());
    let (start, end, status) = resolve_range(range_header, total, is_streamable(content_type));

    // `resolve_range` represents an empty 200 as (0, 0, 200). It is a span
    // sentinel, not one byte to read.
    let length = response_length(total, start, end);
    let mut bytes = vec![0u8; length as usize];
    if length > 0 {
        if let Err(error) = file
            .seek(SeekFrom::Start(start))
            .and_then(|_| file.read_exact(&mut bytes))
        {
            return warn_404("read failed", format!("{path}: {error}"));
        }
    }

    let mut builder = tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", length.to_string());
    if status == 206 {
        builder = builder.header("Content-Range", format!("bytes {start}-{end}/{total}"));
    }
    builder.body(bytes).unwrap_or_else(|_| not_found())
}

/// Serves immutable content-addressed thumbnails, previews, full-resolution
/// stills, and video strip frames. Cache misses are expected 404s.
pub(crate) fn serve_cache(
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let Some(root) = crate::cache_root() else {
        return not_found();
    };
    let cache = crate::preview::CachePaths::new(root);
    let path = request.uri().path().trim_start_matches('/');
    let file = if let Some(hash) = path.strip_prefix("thumb-") {
        cache.thumb(hash)
    } else if let Some(hash) = path.strip_prefix("preview-") {
        cache.preview(hash)
    } else if let Some(hash) = path.strip_prefix("fullres-") {
        cache.fullres(hash)
    } else if let Some(rest) = path.strip_prefix("strip-") {
        match rest.rsplit_once('-') {
            Some((hash, index)) => match index.parse::<u32>() {
                Ok(index) => crate::video::strip_path(&cache, hash, index),
                Err(_) => return not_found(),
            },
            None => return not_found(),
        }
    } else {
        return not_found();
    };
    match std::fs::read(file) {
        Ok(bytes) => {
            let content_type = sniff_image_content_type(&bytes);
            tauri::http::Response::builder()
                .status(200)
                .header("Content-Type", content_type)
                .header("Cache-Control", "public, max-age=31536000, immutable")
                .body(bytes)
                .unwrap_or_else(|_| not_found())
        }
        Err(_) => not_found(),
    }
}

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

/// Length of the resolved response span. `resolve_range` uses `(0, 0)` as the
/// empty-file sentinel, so ordinary inclusive-end arithmetic does not apply.
pub fn response_length(total: u64, start: u64, end: u64) -> u64 {
    if total == 0 {
        0
    } else {
        end - start + 1
    }
}

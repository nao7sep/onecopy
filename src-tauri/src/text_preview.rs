//! Bounded, inert text decoding for indexed Other files.

use std::io::Read;
use std::path::Path;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;
use serde::Serialize;

pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// The hard ceiling an unbounded `textPreviewMaxBytes` setting is clamped to
/// (C-L2): without one, a large stored setting sends a whole file through
/// blake3, decoding and IPC on every preview.
pub const MAX_ALLOWED_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_FALLBACK_ENCODING: &str = "utf-8";

const ENCODINGS: &[&str] = &[
    "utf-8",
    "utf-16le",
    "utf-16be",
    "utf-32le",
    "utf-32be",
    "big5",
    "euc-jp",
    "euc-kr",
    "gb18030",
    "gbk",
    "ibm866",
    "iso-2022-jp",
    "iso-8859-2",
    "iso-8859-3",
    "iso-8859-4",
    "iso-8859-5",
    "iso-8859-6",
    "iso-8859-7",
    "iso-8859-8",
    "iso-8859-8-i",
    "iso-8859-10",
    "iso-8859-13",
    "iso-8859-14",
    "iso-8859-15",
    "iso-8859-16",
    "koi8-r",
    "koi8-u",
    "macintosh",
    "shift_jis",
    "windows-874",
    "windows-1250",
    "windows-1251",
    "windows-1252",
    "windows-1253",
    "windows-1254",
    "windows-1255",
    "windows-1256",
    "windows-1257",
    "windows-1258",
    "x-mac-cyrillic",
    "x-user-defined",
];

/// What Settings offers for the text preview: the fallback encodings and the
/// largest preview limit the reader allows.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    pub encodings: &'static [&'static str],
    pub max_allowed_bytes: u64,
}

pub fn options() -> Options {
    Options {
        encodings: ENCODINGS,
        max_allowed_bytes: MAX_ALLOWED_BYTES,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "body")]
pub enum PreviewBody {
    Text {
        text: String,
        encoding: String,
        /// How automatic decoding picked `encoding` — a Unicode marker, an
        /// exact UTF-8 match, the detector's guess, or the configured
        /// fallback — so the picker can show that uncertainty instead of
        /// implying every automatic result is equally confident
        /// (content-presentation.md D8). `None` for an explicit manual
        /// choice, which carries no such ambiguity.
        method: Option<&'static str>,
        content_key: String,
        encodings: &'static [&'static str],
        byte_size: u64,
    },
    Attributes {
        reason: String,
        /// The condition, when OneCopy authored the reason itself, so the
        /// interface can say it in the reader's language. A decode or I/O
        /// diagnostic has none and shows as recorded.
        reason_code: Option<&'static str>,
        /// The limit the condition names, when it names one.
        reason_bytes: Option<u64>,
        byte_size: u64,
    },
    DecodeError {
        reason: String,
        content_key: String,
        encodings: &'static [&'static str],
        byte_size: u64,
    },
}

/// The saved text-preview limits, clamped to what the reader allows.
pub struct Limits {
    pub max_bytes: u64,
    pub fallback_encoding: String,
}

impl Limits {
    pub fn from_config(config: Option<&serde_json::Value>) -> Self {
        Self {
            max_bytes: config
                .and_then(|value| value.get("textPreviewMaxBytes"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(DEFAULT_MAX_BYTES)
                .clamp(1, MAX_ALLOWED_BYTES),
            fallback_encoding: config
                .and_then(|value| value.get("textFallbackEncoding"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(DEFAULT_FALLBACK_ENCODING)
                .to_string(),
        }
    }
}

pub fn preview_file(
    path: &Path,
    max_bytes: u64,
    fallback: &str,
    requested: Option<&str>,
) -> Result<PreviewBody, String> {
    let max_bytes = max_bytes.clamp(1, MAX_ALLOWED_BYTES);
    let mut file = crate::file_identity::open_regular_nofollow(path)
        .map_err(|error| format!("could not open the indexed file: {error}"))?
        .0;
    let byte_size = file
        .metadata()
        .map_err(|error| format!("could not read file attributes: {error}"))?
        .len();
    if byte_size > max_bytes {
        return Ok(PreviewBody::Attributes {
            reason: format!("Text preview is limited to {max_bytes} bytes."),
            reason_code: Some("preview-too-large"),
            reason_bytes: Some(max_bytes),
            byte_size,
        });
    }
    // The metadata check above is a race, not a bound: a file being written
    // to (a log file kept open, for example) can grow between `metadata()`
    // and this read. `take(max_bytes + 1)` caps the read itself so a growing
    // file can never be decoded past the gate; the `+ 1` distinguishes an
    // exact-fit file from one that has since outgrown the limit (C-L2).
    let mut bytes = Vec::with_capacity(byte_size.min(max_bytes) as usize);
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read the indexed file: {error}"))?;
    if bytes.len() as u64 > max_bytes {
        return Ok(PreviewBody::Attributes {
            reason: format!("Text preview is limited to {max_bytes} bytes."),
            reason_code: Some("preview-too-large"),
            reason_bytes: Some(max_bytes),
            byte_size: bytes.len() as u64,
        });
    }
    let byte_size = bytes.len() as u64;

    let content_key = blake3::hash(&bytes).to_hex().to_string();
    let decoded = match requested {
        Some(label) if !label.eq_ignore_ascii_case("automatic") => {
            canonical_label(label).and_then(|canonical| {
                decode_named(&bytes, canonical).map(|text| (text, canonical.to_string(), None))
            })
        }
        _ => match decode_automatic(&bytes, fallback) {
            Ok(Some(decoded)) => Ok(decoded),
            Ok(None) => {
                return Ok(PreviewBody::Attributes {
                    reason: "The file looks binary rather than textual.".to_string(),
                    reason_code: Some("preview-binary"),
                    reason_bytes: None,
                    byte_size,
                });
            }
            Err(error) => Err(error),
        },
    };
    let decoded = match decoded {
        Ok(decoded) => decoded,
        Err(reason) => {
            return Ok(PreviewBody::DecodeError {
                reason,
                content_key,
                encodings: ENCODINGS,
                byte_size,
            });
        }
    };
    Ok(PreviewBody::Text {
        text: decoded.0,
        encoding: decoded.1,
        method: decoded.2,
        content_key,
        encodings: ENCODINGS,
        byte_size,
    })
}

/// Result tuple: decoded text, canonical encoding, and (for the automatic
/// path only) which of the four steps produced it — see `PreviewBody::Text`.
type Decoded = (String, String, Option<&'static str>);

fn decode_automatic(bytes: &[u8], fallback: &str) -> Result<Option<Decoded>, String> {
    if let Some((encoding, skip)) = unicode_marker(bytes) {
        return decode_named(&bytes[skip..], encoding)
            .map(|text| Some((text, encoding.into(), Some("marker"))));
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        if convincingly_textual(text) {
            return Ok(Some((text.to_string(), "utf-8".to_string(), Some("exact"))));
        }
    }
    if strong_binary_evidence(bytes) {
        return Ok(None);
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let guessed = detector.guess(None, Utf8Detection::Allow);
    let (text, _, had_errors) = guessed.decode(bytes);
    if !had_errors && convincingly_textual(&text) {
        return Ok(Some((
            text.into_owned(),
            guessed.name().to_ascii_lowercase(),
            Some("detected"),
        )));
    }
    let fallback = canonical_label(fallback)?;
    let text = decode_named(bytes, fallback)?;
    Ok(convincingly_textual(&text).then(|| (text, fallback.to_string(), Some("fallback"))))
}

fn canonical_label(label: &str) -> Result<&'static str, String> {
    let normalized = label.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "utf-32le" => Ok("utf-32le"),
        "utf-32be" => Ok("utf-32be"),
        _ => Encoding::for_label(normalized.as_bytes())
            .map(|encoding| encoding.name().to_ascii_lowercase())
            .and_then(|canonical| ENCODINGS.iter().copied().find(|item| *item == canonical))
            .ok_or_else(|| format!("unsupported text encoding: {label}")),
    }
}

/// Decodes under an EXPLICIT choice — a manual selection or the configured
/// fallback, never the automatic detector's own guess (that acceptance stays
/// strict in `decode_automatic`). content-presentation.md: "Invalid byte
/// sequences under a selected fallback render replacement characters rather
/// than crashing or silently discarding bytes" — so an explicit choice always
/// succeeds, substituting U+FFFD for whatever bytes do not fit, rather than
/// turning the whole file into a `DecodeError` over one bad byte.
fn decode_named(bytes: &[u8], label: &str) -> Result<String, String> {
    match canonical_label(label)? {
        "utf-32le" => Ok(decode_utf32(bytes, true)?),
        "utf-32be" => Ok(decode_utf32(bytes, false)?),
        canonical => {
            let encoding = Encoding::for_label(canonical.as_bytes())
                .ok_or_else(|| format!("unsupported text encoding: {label}"))?;
            let (text, _, _had_errors) = encoding.decode(bytes);
            Ok(text.into_owned())
        }
    }
}

/// UTF-32 has no single-byte replacement convention of its own; each
/// four-byte unit that is not a valid code point becomes U+FFFD, matching
/// every other encoding's lossy decode. Only a byte count that is not a
/// multiple of four is a genuine structural failure (there is no unit left to
/// substitute for).
fn decode_utf32(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    if bytes.len() % 4 != 0 {
        return Err("the file length is not valid UTF-32".to_string());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| {
            let code = if little_endian {
                u32::from_le_bytes(chunk.try_into().expect("four-byte chunk"))
            } else {
                u32::from_be_bytes(chunk.try_into().expect("four-byte chunk"))
            };
            char::from_u32(code).unwrap_or('\u{FFFD}')
        })
        .collect())
}

fn unicode_marker(bytes: &[u8]) -> Option<(&'static str, usize)> {
    if bytes.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        Some(("utf-32be", 4))
    } else if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        Some(("utf-32le", 4))
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some(("utf-8", 3))
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Some(("utf-16be", 2))
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Some(("utf-16le", 2))
    } else {
        None
    }
}

fn strong_binary_evidence(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let controls = bytes
        .iter()
        .filter(|byte| matches!(byte, 0..=8 | 11 | 12 | 14..=31 | 127))
        .count();
    bytes.contains(&0) || controls.saturating_mul(100) > bytes.len()
}

fn convincingly_textual(text: &str) -> bool {
    let count = text.chars().count();
    count == 0
        || text
            .chars()
            .filter(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
            .count()
            .saturating_mul(100)
            <= count
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private encoding
// decoders and label canonicalization; promoting them would widen the
// crate's API only for this test.
#[path = "../tests/unit/text_preview.rs"]
mod tests;


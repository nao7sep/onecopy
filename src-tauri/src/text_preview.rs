//! Bounded, inert text decoding for indexed Other files.

use std::io::Read;
use std::path::Path;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;
use serde::Serialize;

pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024;
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

pub fn encodings() -> &'static [&'static str] {
    ENCODINGS
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "body")]
pub enum PreviewBody {
    Text {
        text: String,
        encoding: String,
        content_key: String,
        encodings: &'static [&'static str],
        byte_size: u64,
    },
    Attributes {
        reason: String,
        byte_size: u64,
    },
    DecodeError {
        reason: String,
        content_key: String,
        encodings: &'static [&'static str],
        byte_size: u64,
    },
}

pub fn preview_file(
    path: &Path,
    max_bytes: u64,
    fallback: &str,
    requested: Option<&str>,
) -> Result<PreviewBody, String> {
    let max_bytes = max_bytes.max(1);
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
            byte_size,
        });
    }
    let mut bytes = Vec::with_capacity(byte_size as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("could not read the indexed file: {error}"))?;

    let content_key = blake3::hash(&bytes).to_hex().to_string();
    let decoded = match requested {
        Some(label) if !label.eq_ignore_ascii_case("automatic") => {
            canonical_label(label).and_then(|canonical| {
                decode_named(&bytes, canonical).map(|text| (text, canonical.to_string()))
            })
        }
        _ => match decode_automatic(&bytes, fallback) {
            Ok(Some(decoded)) => Ok(decoded),
            Ok(None) => {
                return Ok(PreviewBody::Attributes {
                    reason: "The file looks binary rather than textual.".to_string(),
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
        content_key,
        encodings: ENCODINGS,
        byte_size,
    })
}

fn decode_automatic(bytes: &[u8], fallback: &str) -> Result<Option<(String, String)>, String> {
    if let Some((encoding, skip)) = unicode_marker(bytes) {
        return decode_named(&bytes[skip..], encoding).map(|text| Some((text, encoding.into())));
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        if convincingly_textual(text) {
            return Ok(Some((text.to_string(), "utf-8".to_string())));
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
        )));
    }
    let fallback = canonical_label(fallback)?;
    let text = decode_named(bytes, fallback)?;
    Ok(convincingly_textual(&text).then(|| (text, fallback.to_string())))
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

fn decode_named(bytes: &[u8], label: &str) -> Result<String, String> {
    match canonical_label(label)? {
        "utf-32le" => decode_utf32(bytes, true),
        "utf-32be" => decode_utf32(bytes, false),
        canonical => {
            let encoding = Encoding::for_label(canonical.as_bytes())
                .ok_or_else(|| format!("unsupported text encoding: {label}"))?;
            let (text, _, had_errors) = encoding.decode(bytes);
            if had_errors {
                Err(format!("the file is not valid {canonical} text"))
            } else {
                Ok(text.into_owned())
            }
        }
    }
}

fn decode_utf32(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    if bytes.len() % 4 != 0 {
        return Err("the file length is not valid UTF-32".to_string());
    }
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let code = if little_endian {
                u32::from_le_bytes(chunk.try_into().expect("four-byte chunk"))
            } else {
                u32::from_be_bytes(chunk.try_into().expect("four-byte chunk"))
            };
            char::from_u32(code).ok_or_else(|| "the file contains invalid UTF-32".to_string())
        })
        .collect()
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
mod tests {
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

    #[test]
    fn every_presented_encoding_is_a_working_canonical_decoder() {
        for label in encodings() {
            assert_eq!(canonical_label(label).unwrap(), *label, "{label}");
            assert!(decode_named(b"", label).is_ok(), "{label}");
        }
    }
}

//! Lossless text at database, JSON and interface boundaries. Native paths stay
//! native during filesystem work. NUL cannot occur in a filename, so it marks
//! invalid native units without reserving any otherwise valid Unicode name.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A readable filename in a diagnostic, never an operand to reopen.
pub fn label(path: &Path) -> impl std::fmt::Display + '_ {
    struct Label<'a>(&'a Path);
    impl std::fmt::Display for Label<'_> {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(&display(&encode(self.0)))
        }
    }
    Label(path)
}

pub fn encode<T: AsRef<OsStr> + ?Sized>(value: &T) -> Cow<'_, str> {
    let value = value.as_ref();
    if let Some(text) = value.to_str() {
        return Cow::Borrowed(text);
    }
    let mut result = String::new();
    #[cfg(unix)]
    {
        use std::fmt::Write;
        use std::os::unix::ffi::OsStrExt;
        let mut bytes = value.as_bytes();
        while !bytes.is_empty() {
            match std::str::from_utf8(bytes) {
                Ok(text) => {
                    result.push_str(text);
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    result.push_str(std::str::from_utf8(&bytes[..valid]).unwrap());
                    let invalid = error.error_len().unwrap_or(bytes.len() - valid);
                    for byte in &bytes[valid..valid + invalid] {
                        write!(result, "\0b{byte:02x}").unwrap();
                    }
                    bytes = &bytes[valid + invalid..];
                }
            }
        }
    }
    #[cfg(windows)]
    {
        use std::fmt::Write;
        use std::os::windows::ffi::OsStrExt;
        for unit in char::decode_utf16(value.encode_wide()) {
            match unit {
                Ok(character) => result.push(character),
                Err(error) => write!(result, "\0w{:04x}", error.unpaired_surrogate()).unwrap(),
            }
        }
    }
    Cow::Owned(result)
}

/// Validate once at externally stored name boundaries. Canonical round trips
/// reject escapes for ASCII, separators, NUL, valid UTF-8 or paired surrogates:
/// none may provide a second spelling for a native name or hide traversal.
pub fn try_from_text(text: &str) -> Option<Cow<'_, Path>> {
    if !text.contains('\0') {
        return Some(Cow::Borrowed(Path::new(text)));
    }
    let path = decode(text)?;
    (encode(&path) == text).then_some(Cow::Owned(path))
}

/// Invalid or foreign-platform escapes retain their NUL and fail at the OS
/// boundary. Never substitute a readable label and risk opening another file.
pub fn from_text(text: &str) -> Cow<'_, Path> {
    try_from_text(text).unwrap_or_else(|| Cow::Borrowed(Path::new(text)))
}

fn decode(text: &str) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let mut bytes = Vec::with_capacity(text.len());
        let mut remaining = text;
        while let Some(index) = remaining.find('\0') {
            bytes.extend_from_slice(remaining[..index].as_bytes());
            let escaped = remaining.get(index + 1..index + 4)?;
            if !escaped.starts_with('b') {
                return None;
            }
            let byte = u8::from_str_radix(&escaped[1..], 16).ok()?;
            if byte == 0 {
                return None;
            }
            bytes.push(byte);
            remaining = &remaining[index + 4..];
        }
        bytes.extend_from_slice(remaining.as_bytes());
        Some(PathBuf::from(OsString::from_vec(bytes)))
    }
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        let mut units = Vec::with_capacity(text.len());
        let mut remaining = text;
        while let Some(index) = remaining.find('\0') {
            units.extend(remaining[..index].encode_utf16());
            let escaped = remaining.get(index + 1..index + 6)?;
            if !escaped.starts_with('w') {
                return None;
            }
            let unit = u16::from_str_radix(&escaped[1..], 16).ok()?;
            if unit == 0 {
                return None;
            }
            units.push(unit);
            remaining = &remaining[index + 6..];
        }
        units.extend(remaining.encode_utf16());
        Some(PathBuf::from(OsString::from_wide(&units)))
    }
}

/// Human-readable only. This value must never be used as a filesystem operand.
pub fn display(text: &str) -> Cow<'_, str> {
    if !text.contains('\0') {
        return Cow::Borrowed(text);
    }
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(index) = remaining.find('\0') {
        result.push_str(&remaining[..index]);
        remaining = &remaining[index + 1..];
        let length = match remaining.as_bytes().first() {
            Some(b'b') => 2,
            Some(b'w') => 4,
            _ => {
                result.push_str("\\0");
                continue;
            }
        };
        if let Some(hex) = remaining
            .get(1..1 + length)
            .filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            result.push_str(if length == 2 { "\\x" } else { "\\u" });
            result.push_str(&hex.to_ascii_uppercase());
            remaining = &remaining[1 + length..];
        } else {
            result.push_str("\\0");
        }
    }
    result.push_str(remaining);
    Cow::Owned(result)
}

#[cfg(test)]
#[path = "../tests/unit/native_path.rs"]
mod tests;

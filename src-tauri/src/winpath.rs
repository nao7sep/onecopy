//! Windows long-path support.
//!
//! Windows caps a classic path at 260 characters. The `\\?\` prefix raises the
//! limit to roughly 32767, and Rust's `std::fs` does NOT add it — so without
//! this a photo sitting beyond the limit cannot be opened, hashed, thumbnailed
//! or indexed at all. It never enters the app, silently. Eleven drives of
//! nested camera dumps and dated backup folders are exactly where such paths
//! accumulate, so this is a READING requirement first and a deleting one
//! second.
//!
//! The grammar is a pure native-unit transform. Its Unicode wrapper
//! (`extended_form`) is compiled and tested on EVERY host, not just Windows
//! — the rules are fiddly enough
//! that leaving them provable only on the machine we visit least would be the
//! worst possible arrangement. `for_fs` is the platform-gated wrapper the
//! filesystem call sites use; on unix it is a no-op that costs nothing.

use std::borrow::Cow;
use std::path::Path;

/// The verbatim prefix, and the UNC form of it.
const VERBATIM: &str = r"\\?\";
const VERBATIM_UNC: &str = r"\\?\UNC\";

/// Rewrites an absolute Windows path into its verbatim (`\\?\`) form.
///
/// Returns `None` when the input must be left exactly as it is:
///
/// - already verbatim (`\\?\…`) — prefixing twice produces a path that resolves
///   to nothing;
/// - a device path (`\\.\…`), which is not a filesystem path at all;
/// - not absolute — a verbatim prefix on a relative path is meaningless, and
///   drive-relative (`C:folder`) is relative despite carrying a drive;
/// - containing a `..` component. Verbatim paths are passed to the filesystem
///   WITHOUT normalization, so `..` would no longer mean "parent" and the path
///   would silently address something else. Refusing is the honest answer; the
///   caller keeps the classic path and the classic limit.
pub fn extended_form(path: &str) -> Option<String> {
    let units: Vec<u16> = path.encode_utf16().collect();
    extended_wide_form(&units).map(|extended| {
        // The core only changes ASCII separators and prefixes, preserving
        // the validity of UTF-16 supplied by a Unicode string.
        String::from_utf16(&extended).expect("path adaptation preserves valid UTF-16")
    })
}

/// Strips a verbatim prefix back off for DISPLAY. Paths reach the user in the
/// metadata pane's copy list and the issues list, and `\\?\C:\photos\a.jpg` is
/// not what anyone recognises as the location of their photo.
///
/// Borrowed in every case but UNC, where the `\\` has to be put back.
pub fn for_display(path: &str) -> Cow<'_, str> {
    if let Some(rest) = path.strip_prefix(VERBATIM_UNC) {
        // \\?\UNC\server\share → \\server\share
        return Cow::Owned(format!(r"\\{rest}"));
    }
    Cow::Borrowed(path.strip_prefix(VERBATIM).unwrap_or(path))
}

/// The form to hand to the filesystem. A no-op everywhere but Windows, so call
/// sites need no `cfg` of their own.
#[cfg(windows)]
pub fn for_fs(path: &Path) -> Cow<'_, Path> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let native: Vec<u16> = path.as_os_str().encode_wide().collect();
    match extended_wide_form(&native) {
        Some(extended) => Cow::Owned(std::path::PathBuf::from(
            std::ffi::OsString::from_wide(&extended),
        )),
        None => Cow::Borrowed(path),
    }
}

/// Unix has no such limit and the prefix is meaningless here, so this is the
/// identity — the call sites stay platform-free.
#[cfg(not(windows))]
pub fn for_fs(path: &Path) -> Cow<'_, Path> {
    Cow::Borrowed(path)
}

/// Operates on native Windows units so long-path adaptation cannot discard
/// an unpaired surrogate in an otherwise usable filename. The Unicode wrapper
/// exercises the same algorithm on every host.
fn extended_wide_form(path: &[u16]) -> Option<Vec<u16>> {
    let slash = u16::from(b'\\');
    let verbatim: Vec<u16> = VERBATIM.encode_utf16().collect();
    let device: Vec<u16> = r"\\.\".encode_utf16().collect();
    if path.starts_with(&verbatim) || path.starts_with(&device) { return None; }
    let normalized: Vec<u16> = path.iter().map(|&unit|
        if unit == u16::from(b'/') { slash } else { unit }).collect();
    if normalized.split(|&unit| unit == slash).any(|part| part == [46, 46]) {
        return None;
    }
    let mut result;
    if normalized.starts_with(&[slash, slash]) {
        let rest = &normalized[2..];
        if rest.is_empty() || !rest.contains(&slash) { return None; }
        result = VERBATIM_UNC.encode_utf16().collect::<Vec<_>>();
        result.extend_from_slice(rest);
    } else {
        if normalized.len() < 3
            || !((65..=90).contains(&normalized[0]) || (97..=122).contains(&normalized[0]))
            || normalized[1] != 58 || normalized[2] != slash { return None; }
        result = verbatim;
        result.extend_from_slice(&normalized);
    }
    Some(result)
}

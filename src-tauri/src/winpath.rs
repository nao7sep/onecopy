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
//! The grammar is a pure string transform (`extended_form`) that is compiled
//! and tested on EVERY host, not just Windows — the rules are fiddly enough
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
    if path.starts_with(VERBATIM) || path.starts_with(r"\\.\") {
        return None;
    }
    // Verbatim paths accept only backslashes; a forward slash stays a literal
    // character in the file name rather than a separator.
    let normalized = path.replace('/', "\\");

    if normalized
        .split('\\')
        .any(|component| component == "..")
    {
        return None;
    }

    if let Some(rest) = normalized.strip_prefix(r"\\") {
        // UNC share: \\server\share\… → \\?\UNC\server\share\…
        // A bare \\server with no share is not a usable root.
        if rest.is_empty() || !rest.contains('\\') {
            return None;
        }
        return Some(format!("{VERBATIM_UNC}{rest}"));
    }

    // Drive absolute: C:\… (a single ASCII letter, a colon, then a separator).
    let bytes = normalized.as_bytes();
    let drive_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\';
    if !drive_absolute {
        return None;
    }
    Some(format!("{VERBATIM}{normalized}"))
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
    match path.to_str().and_then(extended_form) {
        Some(extended) => Cow::Owned(std::path::PathBuf::from(extended)),
        None => Cow::Borrowed(path),
    }
}

/// Unix has no such limit and the prefix is meaningless here, so this is the
/// identity — the call sites stay platform-free.
#[cfg(not(windows))]
pub fn for_fs(path: &Path) -> Cow<'_, Path> {
    Cow::Borrowed(path)
}

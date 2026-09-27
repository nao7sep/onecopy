//! Physical directory relationship checks.
//!
//! Config and native dialogs retain their literal paths. Safety decisions use
//! a separate canonical projection so symlink aliases cannot make a directory
//! inside a scanned source look external.

use std::path::{Path, PathBuf};

fn canonical(path: &Path) -> Result<PathBuf, String> {
    crate::volume_io::canonicalize(path)
        .map_err(|e| format!("could not resolve directory {}: {e}", path.display()))
}

/// Whether `candidate` is `root` or lies beneath it physically. Only a
/// candidate that cannot be resolved is an error: a root that cannot be
/// resolved right now (an unplugged drive, a removed folder) cannot contain
/// anything that exists, so it answers "not within".
pub fn directory_is_within(candidate: &Path, root: &Path) -> Result<bool, String> {
    let candidate = canonical(candidate)?;
    let Ok(root) = canonical(root) else {
        return Ok(false);
    };
    let root_identity = crate::file_identity::FileIdentity::from_path(&root)
        .map_err(|e| format!("could not identify source directory {}: {e}", root.display()))?;
    for ancestor in candidate.ancestors() {
        if crate::file_identity::FileIdentity::from_path(ancestor)
            .is_ok_and(|identity| identity == root_identity)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn directory_is_within_any(candidate: &Path, roots: &[&Path]) -> Result<bool, String> {
    for root in roots {
        if directory_is_within(candidate, root)? {
            return Ok(true);
        }
    }
    Ok(false)
}

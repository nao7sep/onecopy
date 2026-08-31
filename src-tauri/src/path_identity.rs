//! Physical directory relationship checks.
//!
//! Config and native dialogs retain their literal paths. Safety decisions use
//! a separate canonical projection so symlink aliases cannot make a directory
//! inside a scanned source look external.

use std::path::{Path, PathBuf};

fn canonical(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(crate::winpath::for_fs(path).as_ref())
        .map_err(|e| format!("could not resolve directory {}: {e}", path.display()))
}

pub fn directory_is_within(candidate: &Path, root: &Path) -> Result<bool, String> {
    let candidate = canonical(candidate)?;
    let root = canonical(root)?;
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

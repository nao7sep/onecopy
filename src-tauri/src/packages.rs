//! Package directories: folders the system presents as one document or app,
//! such as a Photos or Lightroom library or an application. The files inside
//! one belong to the program that made it, and deleting or moving a
//! "duplicate" photo inside a library damages the library. OneCopy never
//! indexes inside a package, and a source folder inside or equal to one is
//! refused.
//!
//! Packages are recognised by their extension on every platform, so a
//! library copied to a Windows or FAT drive is still one.

use std::path::{Component, Path};

/// Extensions (lowercase, no dot) of the package directories OneCopy leaves
/// alone: photo, video and music libraries, then applications and bundles.
const PACKAGE_EXTENSIONS: &[&str] = &[
    "photoslibrary",
    "photolibrary",
    "migratedphotolibrary",
    "aplibrary",
    "lrlibrary",
    "lrdata",
    "fcpbundle",
    "imovielibrary",
    "tvlibrary",
    "musiclibrary",
    "app",
    "bundle",
    "framework",
    "plugin",
    "appex",
    "kext",
];

/// Whether a directory with this name is a package.
pub fn is_package_name(name: &std::ffi::OsStr) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            PACKAGE_EXTENSIONS
                .iter()
                .any(|package| package.eq_ignore_ascii_case(extension))
        })
}

/// Whether `path` is inside a package directory: any of its folders is one.
/// The last component counts only when `path_is_dir`, since a regular file
/// may carry one of these extensions (a flat installer, say).
pub fn within_package(path: &Path, path_is_dir: bool) -> bool {
    let components: Vec<Component<'_>> = path.components().collect();
    let folders = if path_is_dir { components.len() } else { components.len().saturating_sub(1) };
    components[..folders].iter().any(|component| match component {
        Component::Normal(name) => is_package_name(name),
        _ => false,
    })
}

#[cfg(test)]
#[path = "../tests/unit/packages.rs"]
mod tests;

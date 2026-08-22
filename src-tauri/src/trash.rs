//! The app-managed trash: per-volume, day-foldered, manifest-carrying, and
//! write-only from the app's perspective (the app never purges; deleting any
//! day folder by hand is safe because nothing outside it references its
//! contents — the invariant the design states).
//!
//! Layout on each volume:
//!
//! ```text
//! <trash root>/20260808-utc/manifest.jsonl
//! <trash root>/20260808-utc/<original path relative to the volume root>
//! ```
//!
//! The trash root is `<volume root>/.onecopy-trash` — a trash move is a
//! same-volume rename: instant, zero net space. Exception (a boot-volume fix
//! the design left open): files living on the same volume as the user's home
//! use `~/.onecopy/trash/` instead, because macOS forbids creating entries at
//! `/` — still a same-volume rename, just rooted where the app may write.
//!
//! A stored-name collision (same file re-created and re-trashed the same day)
//! is resolved by a suffix loop plus an atomic exclusive rename
//! (`image1-2.jpg`, …); the manifest line records both the original path and
//! the actual stored name, so restore mapping stays exact in the suffixed case.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::logging;

pub const TRASH_DIR_NAME: &str = ".onecopy-trash";
/// The per-day restore ledger. Named once so the sizing pass can recognise and
/// exclude its own bookkeeping (see `tree_size`).
pub const MANIFEST_FILE_NAME: &str = "manifest.jsonl";
/// The home-volume trash lives under the app root (macOS forbids creating
/// `/.onecopy-trash`). Named once in paths.rs, like every other subpath.
use crate::paths::TRASH_DIR_NAME as HOME_TRASH_SUBDIR;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrashedRecord {
    pub original_path: String,
    pub stored_path: String,
    pub content_hash: Option<String>,
    pub deleted_at_utc: String,
}

/// Moves one file into its volume's trash. `app_root` is the resolved storage
/// root (`~/.onecopy`), used for the home-volume exception; `content_hash` is
/// what the index knows (recorded into the manifest for later audit).
pub fn trash_file(
    file: &Path,
    app_root: &Path,
    content_hash: Option<&str>,
) -> Result<TrashedRecord, String> {
    trash_file_inner(file, app_root, content_hash, |_| {})
}

fn trash_file_inner(
    file: &Path,
    app_root: &Path,
    content_hash: Option<&str>,
    before_move: impl FnOnce(&Path),
) -> Result<TrashedRecord, String> {
    if !file.is_absolute() {
        return Err(format!("trash requires an absolute path: {}", file.display()));
    }
    let source_type = std::fs::symlink_metadata(crate::winpath::for_fs(file).as_ref())
        .map_err(|e| e.to_string())?
        .file_type();
    if !source_type.is_file() {
        return Err(format!(
            "trash source is not a regular file: {}",
            file.display()
        ));
    }

    let volume_root = volume_root_of(file)?;
    let trash_root = trash_root_for(&volume_root, app_root)?;
    // Day folders use the FILENAME timestamp form (`yyyymmdd-utc`), never a
    // slice of the serialized ISO form — the timestamp conventions' date-only
    // grammar, with `-utc` carried because the files inside are the user's
    // own originals and cannot carry it themselves.
    let day = format!("{}-utc", &logging::filename_stamp_now()[..8]);
    let day_dir = trash_root.join(&day);

    // FLAT: the day folder holds file names only, no preserved directory
    // structure. The manifest carries provenance, so the folder can be the
    // plain "everything deleted this day" view an OS trash shows — and a
    // trashed path never grows longer than <trash>/<day>/<name>, which is what
    // stopped the trash amplifying the platform's path-length limit.
    //
    // Still verified as being under its own volume root: the whole point of a
    // per-volume trash is that the move stays a same-volume rename.
    if !path_is_under_volume(file, &volume_root) {
        return Err(format!(
            "{} is not under its own volume root {}",
            file.display(),
            volume_root.display()
        ));
    }
    let name = file
        .file_name()
        .ok_or_else(|| format!("{} has no file name", file.display()))?;
    let target = day_dir.join(name);
    std::fs::create_dir_all(&day_dir).map_err(|e| e.to_string())?;

    let stored = available_stored_path(&target)?;

    let record = TrashedRecord {
        original_path: file.to_string_lossy().to_string(),
        stored_path: stored.to_string_lossy().to_string(),
        content_hash: content_hash.map(|h| h.to_string()),
        deleted_at_utc: logging::now_iso_millis(),
    };
    // Provenance commits FIRST. If append/fsync fails, the indexed source has
    // not moved and remains authoritative. A crash or an exact-boundary target
    // collision after this point can leave a harmless stale audit line, never
    // an untracked file whose original location was lost.
    append_manifest(&day_dir, &record)?;
    crate::fs_publish::sync_directory(&day_dir).map_err(|e| e.to_string())?;
    before_move(&stored);
    crate::fs_publish::rename_no_replace(file, &stored).map_err(|e| {
        format!(
            "trash move failed for {} (source remains in place): {e}",
            file.display()
        )
    })?;
    let _ = crate::fs_publish::sync_directory(&day_dir);

    #[cfg(windows)]
    hide_windows(&trash_root);

    Ok(record)
}

/// Selects `target`, falling back to `stem-2.ext`, `stem-3.ext`, … when the
/// name is occupied. Final authority is the later atomic exclusive rename: an
/// external exact-boundary winner is preserved and the source stays put.
fn available_stored_path(target: &Path) -> Result<PathBuf, String> {
    let mut candidate = target.to_path_buf();
    let mut counter = 2u32;
    loop {
        match std::fs::symlink_metadata(&candidate) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(err) => return Err(err.to_string()),
        }
        // A runaway guard, not a design limit: a hyphen and a number stay
        // short even at a million, and real collisions are rare.
        if counter > 1_000_000 {
            return Err(format!(
                "could not find a free trash name for {}",
                target.display()
            ));
        }
        candidate = suffixed_name(target, counter);
        counter += 1;
    }
}

/// `image1.jpg` + 2 → `image1-2.jpg`; extensionless names get `name-2`.
fn suffixed_name(target: &Path, counter: u32) -> PathBuf {
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    // Hyphen, never a period: a period reads as a second extension, and a
    // repeated separator would grow the name without bound.
    let name = match target.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}-{counter}.{ext}"),
        None => format!("{stem}-{counter}"),
    };
    target.with_file_name(name)
}

/// Appends one manifest line (JSONL). The manifest lives INSIDE the day folder
/// it describes, keeping the folder self-contained and hand-deletable.
fn append_manifest(day_dir: &Path, record: &TrashedRecord) -> Result<(), String> {
    let line = serde_json::to_string(record).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(day_dir.join(MANIFEST_FILE_NAME))
        .map_err(|e| e.to_string())?;
    // not recorded: the manifest is trash-side audit data, append-mode by
    // construction, never managed text.
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

#[cfg(test)]
mod transaction_tests {
    // EXCEPTION to tests-folder conventions: the callback is a private
    // exact-boundary seam and must not widen the shipped Trash API.
    use super::*;

    #[test]
    fn exact_boundary_winner_survives_and_source_remains_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let app_root = dir.path().join("app");
        let source_dir = dir.path().join("source");
        std::fs::create_dir_all(&app_root).unwrap();
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("photo.jpg");
        std::fs::write(&source, b"source").unwrap();

        let result = trash_file_inner(&source, &app_root, None, |target| {
            std::fs::write(target, b"winner").unwrap();
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"source");
        let day = std::fs::read_dir(app_root.join("trash"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read(day.join("photo.jpg")).unwrap(), b"winner");
    }
}

/// The trash root for a volume: `<volume root>/.onecopy-trash`, except the
/// home volume, which uses `<app root>/trash` (macOS forbids writing at `/`).
///
/// `pub` for the tests: the volume root is already a parameter, so passing an
/// arbitrary one is the whole seam — every test runs on the home volume, so
/// the external-volume branch would otherwise never execute, though culling on
/// an SD card or a backup drive takes it on every single delete.
pub fn trash_root_for(volume_root: &Path, app_root: &Path) -> Result<PathBuf, String> {
    let home_volume = dirs_home()
        .and_then(|home| volume_root_of(&home).ok())
        .map(|root| root == volume_root)
        .unwrap_or(false);
    if home_volume {
        Ok(app_root.join(HOME_TRASH_SUBDIR))
    } else {
        Ok(volume_root.join(TRASH_DIR_NAME))
    }
}

/// One trash root's standing facts for the Trash surface: where it is, how
/// much it holds. Sizes are computed on demand — the surface opens rarely and
/// a cached number would only be a chance to lie.
#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrashRootInfo {
    pub root: String,
    pub bytes: u64,
    pub files: u64,
}

/// Every trash root the configured source directories imply (their volumes,
/// deduplicated) plus the app-home trash, each with its current size. A root
/// that does not exist yet reports zero rather than being omitted — the row
/// tells the user where trash WOULD go, which is standing state too.
pub fn overview(source_dirs: &[String], app_root: &Path) -> Vec<TrashRootInfo> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for dir in source_dirs {
        if let Ok(volume) = volume_root_of(Path::new(dir)) {
            if let Ok(root) = trash_root_for(&volume, app_root) {
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
    }
    let home = app_root.join(HOME_TRASH_SUBDIR);
    if !roots.contains(&home) {
        roots.push(home);
    }
    // Every MOUNTED volume is also probed, so a trash left behind on a drive
    // no longer configured as a source still appears here (and only here —
    // the overview is the single authority Empty verifies against).
    for root in mounted_trash_roots() {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
        .into_iter()
        .map(|root| {
            let (bytes, files) = tree_size(&root);
            TrashRootInfo {
                root: root.to_string_lossy().to_string(),
                bytes,
                files,
            }
        })
        .collect()
}

/// Trash roots on the volumes mounted under `volumes` — the pure, testable
/// half of mounted-volume discovery. Presence-only and read-cheap: one
/// existence probe per volume, no sizing. Symlinked entries are skipped
/// (macOS keeps a boot-volume symlink in /Volumes, and the boot volume's
/// trash lives in the app root, not at `/`). Sorted for a stable overview.
pub fn discover_in_volumes_dir(volumes: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(volumes) else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .symlink_metadata()
                .map(|meta| meta.file_type().is_dir())
                .unwrap_or(false)
        })
        .map(|entry| entry.path().join(TRASH_DIR_NAME))
        .filter(|candidate| candidate.is_dir())
        .collect();
    roots.sort();
    roots
}

/// The platform's mounted volumes: /Volumes on macOS, present drive letters
/// on Windows (an absent letter fails its probe instantly).
#[cfg(unix)]
fn mounted_trash_roots() -> Vec<PathBuf> {
    discover_in_volumes_dir(Path::new("/Volumes"))
}

#[cfg(windows)]
fn mounted_trash_roots() -> Vec<PathBuf> {
    ('A'..='Z')
        .map(|letter| PathBuf::from(format!("{letter}:\\{TRASH_DIR_NAME}")))
        .filter(|candidate| candidate.is_dir())
        .collect()
}

/// Empties one trash root by deleting its day folders. PERMANENT by nature —
/// the caller confirms with the totals first — and the root itself stays so
/// the next trash move needs no re-setup. A file that refuses deletion is
/// simply left (reported in the count difference); the trash never needs to
/// be perfect, only smaller.
pub fn empty_root(root: &Path) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    let entries = std::fs::read_dir(root).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            // Stray top-level files (none are written today) go too: the
            // user asked for empty.
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

/// Total bytes and file count of the RECOVERABLE contents of a tree; a missing
/// tree is (0, 0).
///
/// The per-day `manifest.jsonl` is excluded. It is our own bookkeeping, not
/// something the user put in the trash, and counting it made the overview
/// disagree with itself: a trash holding two deleted photos read "3 files",
/// and a trash emptied of everything recoverable could still read "1 file" —
/// with no way to reach zero. The count answers "how much of my library is in
/// here", so only entries a restore could hand back may contribute.
fn tree_size(root: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    for entry in walkdir::WalkDir::new(root).follow_links(false).into_iter().flatten() {
        if entry.file_type().is_file() && entry.file_name() != MANIFEST_FILE_NAME {
            files += 1;
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (bytes, files)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The volume (mount point / drive) root containing `path`.
#[cfg(unix)]
pub fn volume_root_of(path: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    let start = nearest_existing(path);
    let dev = std::fs::metadata(&start).map_err(|e| e.to_string())?.dev();
    let mut current = start;
    loop {
        let Some(parent) = current.parent() else {
            return Ok(current); // reached `/`
        };
        let parent_dev = std::fs::metadata(parent).map_err(|e| e.to_string())?.dev();
        if parent_dev != dev {
            return Ok(current); // crossing here changes device: current is the mount point
        }
        current = parent.to_path_buf();
    }
}

/// On Windows the volume root is the path's prefix (drive letter or UNC share).
#[cfg(windows)]
pub fn volume_root_of(path: &Path) -> Result<PathBuf, String> {
    // WalkDir inherits the verbatim form from a long-path root, so indexed
    // paths arrive here as `\\?\C:\…` (or `\\?\UNC\…`). The Prefix component
    // of that spelling is itself verbatim; joining it produced `\\?\C:\` and
    // made the home-volume comparison fail, routing deletes into C:\.onecopy-trash.
    // Strip only the namespace marker before deriving the ordinary drive/share
    // root. Filesystem calls still receive the verbatim spelling through for_fs.
    let raw = path.to_string_lossy();
    let conventional = crate::winpath::for_display(&raw);
    let mut components = Path::new(conventional.as_ref()).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Prefix(prefix)), Some(std::path::Component::RootDir)) => {
            Ok(PathBuf::from(prefix.as_os_str()).join(std::path::MAIN_SEPARATOR.to_string()))
        }
        _ => Err(format!("no volume prefix in {}", path.display())),
    }
}

#[cfg(windows)]
fn path_is_under_volume(file: &Path, volume_root: &Path) -> bool {
    let raw = file.to_string_lossy();
    let conventional = crate::winpath::for_display(&raw);
    Path::new(conventional.as_ref()).starts_with(volume_root)
}

#[cfg(not(windows))]
fn path_is_under_volume(file: &Path, volume_root: &Path) -> bool {
    file.starts_with(volume_root)
}

// Walks up to the nearest existing ancestor, so a just-deleted sibling or a
// not-yet-created leaf never breaks volume detection. Unix-only: the Windows
// `volume_root_of` reads the path prefix and never touches the filesystem.
#[cfg(unix)]
fn nearest_existing(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    while !current.exists() {
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }
    current
}

#[cfg(windows)]
fn hide_windows(trash_root: &Path) {
    // Best-effort: mark the trash root hidden (dot-prefix means nothing to
    // Explorer). attrib +h via cmd avoids a winapi dependency for one flag.
    let _ = std::process::Command::new("attrib")
        .arg("+h")
        .arg(trash_root)
        .status();
}

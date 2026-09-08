//! Recoverable deleted-file storage: root-local, day-foldered, and
//! write-only from the app's perspective (the app never purges; deleting any
//! day folder by hand is safe because nothing outside it references its
//! contents — the invariant the design states).
//!
//! Layout beneath each configured source or destination root:
//!
//! ```text
//! <configured root>/.onecopy-trash/20260808-utc/manifest.jsonl
//! <configured root>/.onecopy-trash/20260808-utc/<stored file>
//! ```
//!
//! The owning configured root is frozen before execution. The storage boundary
//! proves physical containment and same-filesystem placement before creating
//! the hidden directory or moving the file. Different roots therefore never
//! share a permission boundary.
//!
//! A stored-name collision (same file re-created and re-trashed the same day)
//! is resolved by a suffix loop plus an atomic exclusive rename
//! (`image1-2.jpg`, …); the manifest line records both the original path and
//! the actual stored name, so restore mapping stays exact in the suffixed case.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::logging;

pub const TRASH_DIR_NAME: &str = ".onecopy-trash";
/// The per-day restore ledger. Named once so the sizing pass can recognise and
/// exclude its own bookkeeping (see `tree_size`).
pub const MANIFEST_FILE_NAME: &str = "manifest.jsonl";

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrashedRecord {
    pub original_path: String,
    pub stored_path: String,
    pub content_hash: Option<String>,
    pub deleted_at_utc: String,
}

/// Moves one file beneath the frozen configured root that owns it.
pub fn trash_file(
    file: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
) -> Result<TrashedRecord, String> {
    trash_file_with_before_move(file, owning_root, content_hash, |_| {})
}

fn trash_file_with_before_move(
    file: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
    before_move: impl FnOnce(&Path),
) -> Result<TrashedRecord, String> {
    if !file.is_absolute() {
        return Err(format!(
            "trash requires an absolute path: {}",
            file.display()
        ));
    }
    let metadata = std::fs::symlink_metadata(crate::winpath::for_fs(file).as_ref())
        .map_err(|error| format!("trash source is unavailable: {error}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "trash source is not a regular file: {}",
            file.display()
        ));
    }
    let plan = prepare_trash(file, owning_root, content_hash)?;
    commit_trash(file, plan, before_move)
}

struct TrashPlan {
    record: TrashedRecord,
    stored: PathBuf,
    day_dir: PathBuf,
    #[cfg(windows)]
    trash_root: PathBuf,
}

fn prepare_trash(
    original: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
) -> Result<TrashPlan, String> {
    if !owning_root.is_absolute() || !owning_root.is_dir() {
        return Err(format!(
            "deleted-file root is unavailable: {}",
            owning_root.display()
        ));
    }
    if !crate::path_identity::directory_is_within(original, owning_root)? {
        return Err(format!(
            "{} is outside its frozen configured root {}",
            original.display(),
            owning_root.display()
        ));
    }
    if volume_root_of(original)? != volume_root_of(owning_root)? {
        return Err("recoverable deletion must stay on the same filesystem".to_string());
    }
    let trash_root = owning_root.join(TRASH_DIR_NAME);
    if crate::path_identity::directory_is_within(original, &trash_root).unwrap_or(false) {
        return Err(
            "a file already in deleted-file storage cannot be deleted into itself".to_string(),
        );
    }
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
    let name = original
        .file_name()
        .ok_or_else(|| format!("{} has no file name", original.display()))?;
    let target = day_dir.join(name);
    std::fs::create_dir_all(&day_dir).map_err(|e| e.to_string())?;

    let stored = available_stored_path(&target)?;

    let record = TrashedRecord {
        original_path: original.to_string_lossy().to_string(),
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

    Ok(TrashPlan {
        record,
        stored,
        day_dir,
        #[cfg(windows)]
        trash_root,
    })
}

fn commit_trash(
    source: &Path,
    plan: TrashPlan,
    before_move: impl FnOnce(&Path),
) -> Result<TrashedRecord, String> {
    before_move(&plan.stored);
    crate::fs_publish::rename_no_replace(source, &plan.stored)
        .map_err(|e| format!("trash move failed for {}: {e}", source.display()))?;
    if let Err(error) = crate::fs_publish::sync_directory(&plan.day_dir) {
        crate::logging::warn(
            "trash directory sync failed after the move completed",
            json!({
                "path": plan.day_dir,
                "error": { "message": error.to_string() },
            }),
        );
    }

    #[cfg(windows)]
    hide_windows(&plan.trash_root);

    Ok(plan.record)
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
mod boundary_tests {
    // EXCEPTION to tests-folder conventions: the callback is a private
    // exact-boundary seam and must not widen the shipped Trash API.
    use super::*;

    #[test]
    fn exact_boundary_winner_survives_and_source_remains_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("source");
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("photo.jpg");
        std::fs::write(&source, b"source").unwrap();

        let result = trash_file_with_before_move(&source, &source_dir, None, |target| {
            std::fs::write(target, b"winner").unwrap()
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"source");
        let day = std::fs::read_dir(source_dir.join(TRASH_DIR_NAME))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read(day.join("photo.jpg")).unwrap(), b"winner");
    }

    #[test]
    fn replacement_before_the_move_is_the_file_that_gets_trashed() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("source");
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("photo.jpg");
        let held = source_dir.join("held.jpg");
        std::fs::write(&source, b"original").unwrap();

        let result = trash_file_with_before_move(&source, &source_dir, None, |_| {
            std::fs::rename(&source, &held).unwrap();
            std::fs::write(&source, b"replacement").unwrap();
        })
        .unwrap();

        assert!(!source.exists());
        assert_eq!(std::fs::read(&held).unwrap(), b"original");
        assert_eq!(std::fs::read(result.stored_path).unwrap(), b"replacement");
    }
}

/// Selects the most-specific configured root containing a planned file. A
/// nested root owns its own deleted files instead of leaking them into an
/// ancestor root with potentially broader permissions.
pub fn root_for_file(file: &Path, configured_roots: &[PathBuf]) -> Result<PathBuf, String> {
    if !file.is_absolute() {
        return Err(format!("{} is not an absolute path", file.display()));
    }
    configured_roots
        .iter()
        .filter(|root| {
            if !root.is_absolute() {
                return false;
            }
            match crate::path_identity::directory_is_within(file, root) {
                Ok(within) => within,
                // A source may disappear after it was indexed. Planning still
                // freezes its configured lexical owner so execution can report
                // that file as one ordinary unavailable target. No move can
                // occur until trash_file revalidates the live physical path.
                Err(_) => file.starts_with(root),
            }
        })
        .max_by_key(|root| root.components().count())
        .cloned()
        .ok_or_else(|| format!("{} is outside every configured root", file.display()))
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

/// Every configured source and destination root has one local deleted-files
/// directory. Missing directories report zero and are created only when the
/// user deletes a file or explicitly reveals that location.
pub fn overview(configured_roots: &[PathBuf]) -> Vec<TrashRootInfo> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for configured in configured_roots {
        let root = configured.join(TRASH_DIR_NAME);
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

/// Validates one UI-selected deleted-files location against the current
/// configuration, creates it when still empty/missing, and re-checks physical
/// containment so a substituted symlink cannot escape the configured root.
pub fn ensure_root_for_reveal(
    configured_roots: &[PathBuf],
    requested: &Path,
) -> Result<PathBuf, String> {
    let owning_root = configured_roots
        .iter()
        .find(|root| root.join(TRASH_DIR_NAME) == requested)
        .ok_or_else(|| "not a known deleted-files location".to_string())?;
    std::fs::create_dir_all(requested)
        .map_err(|error| format!("could not create deleted-files location: {error}"))?;
    let metadata = std::fs::symlink_metadata(requested)
        .map_err(|error| format!("deleted-files location is unavailable: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("deleted-files location is not a real directory".to_string());
    }
    if !crate::path_identity::directory_is_within(requested, owning_root)? {
        return Err("deleted-files location escaped its configured root".to_string());
    }
    Ok(requested.to_path_buf())
}

/// Empties one trash root by deleting its day folders. PERMANENT by nature —
/// the caller confirms with the totals first — and the root itself stays so
/// the next trash move needs no re-setup. A file that refuses deletion is
/// simply left (reported in the count difference); the trash never needs to
/// be perfect, only smaller.
pub fn empty_root(root: &Path) -> Result<(), String> {
    let never_cancelled = AtomicBool::new(false);
    empty_root_with_progress(root, &never_cancelled, &|_| {}, &|_, _| Ok(())).map(|_| ())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyProgress {
    pub done: u64,
    pub total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub failures: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyOutcome {
    pub cancelled: bool,
    pub failures: u64,
}

/// Permanently removes one already-authorized trash root with progress over
/// recoverable files. Manifests are bookkeeping and do not inflate the same
/// totals the overview/confirmation shows. The root itself must remain a real
/// directory and inner symlinks are never followed. Cancellation is checked
/// while planning and between files; an individual filesystem deletion is
/// already atomic at that unit.
pub fn empty_root_with_progress(
    root: &Path,
    cancelled: &AtomicBool,
    progress: &dyn Fn(EmptyProgress),
    record_failure: &dyn Fn(&Path, &str) -> Result<(), String>,
) -> Result<EmptyOutcome, String> {
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err("trash root is not a directory".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            progress(EmptyProgress::default());
            return Ok(EmptyOutcome::default());
        }
        Err(error) => return Err(error.to_string()),
    }

    let mut files: Vec<(PathBuf, u64, bool)> = Vec::new();
    let mut directories: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(EmptyOutcome {
                cancelled: true,
                failures: 0,
            });
        }
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.path() == root {
            continue;
        }
        if entry.file_type().is_dir() {
            directories.push(entry.path().to_path_buf());
        } else {
            // Symlinks and other stray non-directories are bookkeeping, never
            // followed and never counted as recoverable media, but Empty must
            // still remove their directory entries.
            let recoverable =
                entry.file_type().is_file() && entry.file_name() != MANIFEST_FILE_NAME;
            let bytes = recoverable
                .then(|| entry.metadata().map(|metadata| metadata.len()).unwrap_or(0))
                .unwrap_or(0);
            files.push((entry.path().to_path_buf(), bytes, recoverable));
        }
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));

    let mut snapshot = EmptyProgress {
        total: files
            .iter()
            .filter(|(_, _, recoverable)| *recoverable)
            .count() as u64,
        bytes_total: files
            .iter()
            .filter(|(_, _, recoverable)| *recoverable)
            .map(|(_, bytes, _)| *bytes)
            .sum(),
        ..EmptyProgress::default()
    };
    progress(snapshot.clone());

    for (path, bytes, recoverable) in files {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(EmptyOutcome {
                cancelled: true,
                failures: snapshot.failures,
            });
        }
        if let Err(error) = std::fs::remove_file(&path) {
            snapshot.failures += 1;
            record_failure(&path, &error.to_string())?;
            crate::logging::warn(
                "trash entry removal failed",
                serde_json::json!({
                    "path": path,
                    "error": { "message": error.to_string() },
                }),
            );
        }
        if recoverable {
            snapshot.done += 1;
            snapshot.bytes_done = snapshot.bytes_done.saturating_add(bytes);
            progress(snapshot.clone());
        }
    }
    for directory in directories {
        if let Err(error) = std::fs::remove_dir(&directory) {
            if !matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) {
                record_failure(&directory, &error.to_string())?;
                crate::logging::warn(
                    "trash directory cleanup failed",
                    serde_json::json!({
                        "path": directory,
                        "error": { "message": error.to_string() },
                    }),
                );
            }
        }
    }
    Ok(EmptyOutcome {
        cancelled: false,
        failures: snapshot.failures,
    })
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
    // A trash root is created lazily by the first delete. Until then its
    // absence is the ordinary empty state promised by `overview`, not a walk
    // failure worth surfacing in the application log.
    if !root.exists() {
        return (0, 0);
    }

    let mut bytes = 0u64;
    let mut files = 0u64;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                crate::logging::warn(
                    "trash size walk failed",
                    json!({ "path": root, "error": { "message": error.to_string() } }),
                );
                continue;
            }
        };
        if entry.file_type().is_file() && entry.file_name() != MANIFEST_FILE_NAME {
            files += 1;
            match entry.metadata() {
                Ok(metadata) => bytes += metadata.len(),
                Err(error) => crate::logging::warn(
                    "trash file metadata read failed",
                    json!({ "path": entry.path(), "error": { "message": error.to_string() } }),
                ),
            }
        }
    }
    (bytes, files)
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
    match std::process::Command::new("attrib")
        .arg("+h")
        .arg(trash_root)
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => crate::logging::warn(
            "trash directory could not be hidden",
            serde_json::json!({ "path": trash_root, "status": status.code() }),
        ),
        Err(error) => crate::logging::warn(
            "trash directory could not be hidden",
            serde_json::json!({
                "path": trash_root,
                "error": { "message": error.to_string() },
            }),
        ),
    }
}

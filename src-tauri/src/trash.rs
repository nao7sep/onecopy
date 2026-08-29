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

use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;

use crate::logging;

pub const TRASH_DIR_NAME: &str = ".onecopy-trash";
/// The per-day restore ledger. Named once so the sizing pass can recognise and
/// exclude its own bookkeeping (see `tree_size`).
pub const MANIFEST_FILE_NAME: &str = "manifest.jsonl";

static EMPTY_RUNNING: AtomicBool = AtomicBool::new(false);
static EMPTY_CANCELLED: AtomicBool = AtomicBool::new(false);
static EMPTY_TRANSITION: Mutex<()> = Mutex::new(());

pub struct EmptyClaim;

impl EmptyClaim {
    pub fn cancellation_flag(&self) -> &AtomicBool {
        &EMPTY_CANCELLED
    }
}

impl Drop for EmptyClaim {
    fn drop(&mut self) {
        let _transition = EMPTY_TRANSITION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        EMPTY_RUNNING.store(false, Ordering::SeqCst);
    }
}

pub fn begin_empty() -> Result<EmptyClaim, String> {
    let _transition = EMPTY_TRANSITION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if EMPTY_RUNNING.load(Ordering::SeqCst) {
        return Err("a trash root is already being emptied".to_string());
    }
    EMPTY_CANCELLED.store(false, Ordering::SeqCst);
    EMPTY_RUNNING.store(true, Ordering::SeqCst);
    Ok(EmptyClaim)
}

pub fn cancel_empty() -> bool {
    let _transition = EMPTY_TRANSITION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !EMPTY_RUNNING.load(Ordering::SeqCst) {
        return false;
    }
    EMPTY_CANCELLED.store(true, Ordering::SeqCst);
    true
}
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
    trash_file_with_before_move(file, app_root, content_hash, |_| {})
}

fn trash_file_with_before_move(
    file: &Path,
    app_root: &Path,
    content_hash: Option<&str>,
    before_move: impl FnOnce(&Path),
) -> Result<TrashedRecord, String> {
    if !file.is_absolute() {
        return Err(format!("trash requires an absolute path: {}", file.display()));
    }
    let metadata = std::fs::symlink_metadata(crate::winpath::for_fs(file).as_ref())
        .map_err(|error| format!("trash source is unavailable: {error}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("trash source is not a regular file: {}", file.display()));
    }
    let plan = prepare_trash(file, app_root, content_hash)?;
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
    app_root: &Path,
    content_hash: Option<&str>,
) -> Result<TrashPlan, String> {
    let volume_root = volume_root_of(original)?;
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
    if !path_is_under_volume(original, &volume_root) {
        return Err(format!(
            "{} is not under its own volume root {}",
            original.display(),
            volume_root.display()
        ));
    }
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
    crate::fs_publish::rename_no_replace(source, &plan.stored).map_err(|e| {
        format!(
            "trash move failed for {}: {e}",
            source.display()
        )
    })?;
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
        let app_root = dir.path().join("app");
        let source_dir = dir.path().join("source");
        std::fs::create_dir_all(&app_root).unwrap();
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("photo.jpg");
        std::fs::write(&source, b"source").unwrap();

        let result = trash_file_with_before_move(
            &source,
            &app_root,
            None,
            |target| std::fs::write(target, b"winner").unwrap(),
        );

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

    #[test]
    fn replacement_before_the_move_is_the_file_that_gets_trashed() {
        let dir = tempfile::tempdir().unwrap();
        let app_root = dir.path().join("app");
        let source_dir = dir.path().join("source");
        std::fs::create_dir_all(&app_root).unwrap();
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("photo.jpg");
        let held = source_dir.join("held.jpg");
        std::fs::write(&source, b"original").unwrap();

        let result = trash_file_with_before_move(&source, &app_root, None, |_| {
            std::fs::rename(&source, &held).unwrap();
            std::fs::write(&source, b"replacement").unwrap();
        })
        .unwrap();

        assert!(!source.exists());
        assert_eq!(std::fs::read(&held).unwrap(), b"original");
        assert_eq!(std::fs::read(result.stored_path).unwrap(), b"replacement");
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
        match volume_root_of(Path::new(dir)).and_then(|volume| trash_root_for(&volume, app_root)) {
            Ok(root) if !roots.contains(&root) => roots.push(root),
            Ok(_) => {}
            Err(error) => crate::logging::warn(
                "trash root resolution failed",
                json!({ "path": dir, "error": { "message": error } }),
            ),
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
    let entries = match std::fs::read_dir(volumes) {
        Ok(entries) => entries,
        Err(error) => {
            crate::logging::warn(
                "mounted-volume discovery failed",
                json!({ "path": volumes, "error": { "message": error.to_string() } }),
            );
            return Vec::new();
        }
    };
    let mut roots = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                crate::logging::warn(
                    "mounted-volume entry read failed",
                    json!({ "path": volumes, "error": { "message": error.to_string() } }),
                );
                continue;
            }
        };
        let path = entry.path();
        let is_directory = match path.symlink_metadata() {
            Ok(metadata) => metadata.file_type().is_dir(),
            Err(error) => {
                crate::logging::warn(
                    "mounted-volume metadata read failed",
                    json!({ "path": path, "error": { "message": error.to_string() } }),
                );
                continue;
            }
        };
        if is_directory {
            let candidate = path.join(TRASH_DIR_NAME);
            if candidate.is_dir() {
                roots.push(candidate);
            }
        }
    }
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
    let never_cancelled = AtomicBool::new(false);
    empty_root_with_progress(root, &never_cancelled, &|_| {}).map(|_| ())
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
            let recoverable = entry.file_type().is_file()
                && entry.file_name() != MANIFEST_FILE_NAME;
            let bytes = recoverable
                .then(|| entry.metadata().map(|metadata| metadata.len()).unwrap_or(0))
                .unwrap_or(0);
            files.push((entry.path().to_path_buf(), bytes, recoverable));
        }
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));

    let mut snapshot = EmptyProgress {
        total: files.iter().filter(|(_, _, recoverable)| *recoverable).count() as u64,
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

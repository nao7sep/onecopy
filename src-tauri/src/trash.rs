//! Recoverable deleted files stay on the source filesystem. New day folders
//! are private to the deleting OS account; old shared days are left untouched.
//! Layout: <root>/.onecopy-trash/private-<account>--<yyyymmdd-utc>/<file>.
//! Each day carries its own append-only manifest for Browse and Restore.
//! Explicit Empty removes reviewed files and then their manifests; there is
//! no automatic purge. Different app data folders under the same OS account
//! can recover these files without relying on the old app data folder.
//!
//! Unix moves into an owner-only directory through its held descriptor.
//! Windows copies to a new protected file, verifies and flushes it, publishes
//! without replacement, and only then removes the unchanged original. This
//! needs temporary free space and copying time, and leaves live hard links'
//! permissions unchanged. Filesystems unable to enforce privacy refuse new
//! recoverable deletions. Other accounts' private days are outside Browse and
//! Empty; old shared days retain their original access and recovery behavior.
//!
//! Stored-name collisions use suffixes and atomic exclusive publication. The
//! manifest commits first, so interruption never loses the original location.

use std::path::{Path, PathBuf};

use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::formats;
use crate::logging;
use crate::volume_io;

pub const TRASH_DIR_NAME: &str = ".onecopy-trash";

/// Deleted material is never source inventory or a browsable destination.
/// Match complete native path components, not unrelated names containing the
/// reserved name. This boundary is independent of user visibility preferences.
pub fn is_trash_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().eq_ignore_ascii_case(TRASH_DIR_NAME))
}
/// The per-day restore ledger. Named once so the sizing pass can recognise and
/// exclude its own bookkeeping (see `measure_root`).
pub const MANIFEST_FILE_NAME: &str = "manifest.jsonl";

/// Why a file went to Deleted files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrashKind {
    Delete,
    MoveCleanup,
    OverwriteDisplaced,
}

/// Whether a stored file was a main copy or a companion of its item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrashRole {
    Main,
    Companion,
}

/// What the caller knows about one file it sends to Deleted files: the kind
/// of operation, that operation's id, the logical item the file belonged to
/// (none for a displaced destination file, which was never indexed), whether
/// it was a main copy or a companion, and for Move cleanup the output that
/// replaced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashContext {
    pub kind: TrashKind,
    pub operation: String,
    pub item: Option<String>,
    pub role: TrashRole,
    pub moved_to: Option<String>,
}

impl TrashContext {
    pub fn new(kind: TrashKind, operation: &str) -> Self {
        Self {
            kind,
            operation: operation.to_string(),
            item: None,
            role: TrashRole::Main,
            moved_to: None,
        }
    }

    pub fn item(mut self, item: Option<String>) -> Self {
        self.item = item;
        self
    }

    pub fn role(mut self, role: TrashRole) -> Self {
        self.role = role;
        self
    }

    pub fn moved_to(mut self, moved_to: Option<String>) -> Self {
        self.moved_to = moved_to;
        self
    }
}

/// One manifest line, carrying its format version (`formats`).
/// `storedName` and `originalRelative` are relative, so a record still
/// resolves after the root is renamed or its drive is mounted elsewhere;
/// `size` and `mtimeMs` let a reader prove the stored file is the one the
/// record describes. The absolute paths are kept as they were.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrashedRecord {
    pub format_version: i64,
    pub original_path: String,
    pub stored_path: String,
    pub content_hash: Option<String>,
    pub deleted_at_utc: String,
    pub stored_name: String,
    /// `/`-joined components of the original path relative to the owning
    /// root.
    pub original_relative: String,
    pub kind: TrashKind,
    pub operation: String,
    pub item: Option<String>,
    pub role: TrashRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved_to: Option<String>,
    pub size: u64,
    pub mtime_ms: i64,
}

/// Why a file was not moved to Deleted files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashError {
    pub message: String,
    /// The move itself was given up on while its volume was not responding:
    /// the file may be in Deleted files or still in place. Its manifest line
    /// is already written, so it is recoverable either way, and the next
    /// source check settles the index.
    pub outcome_unknown: bool,
    /// Source-read proof no longer names this version. This covers Move
    /// cleanup and the verified private copy required by Windows deletion.
    pub source_changed: bool,
    /// Move delivery no longer authorizes removing any further originals.
    pub destination_changed: bool,
    /// Preparation ended before source removal because the caller cancelled.
    pub cancelled: bool,
}

impl From<String> for TrashError {
    fn from(message: String) -> Self {
        Self {
            message,
            outcome_unknown: false,
            source_changed: false,
            destination_changed: false,
            cancelled: false,
        }
    }
}

impl std::fmt::Display for TrashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Stores one file for recovery beneath the frozen configured root that owns it.
pub fn trash_file(
    file: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
    context: &TrashContext,
) -> Result<TrashedRecord, TrashError> {
    trash_file_with_before_move(file, owning_root, content_hash, context, |_| {})
}

pub(crate) fn trash_file_cancellable(
    file: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
    context: &TrashContext,
    source_version: Option<crate::file_identity::FileVersion>,
    destination: Option<crate::file_identity::VerifiedDestination>,
    cancelled: &dyn Fn() -> bool,
) -> Result<TrashedRecord, TrashError> {
    trash_file_checked(file, owning_root, content_hash, context, source_version, destination, |_| {}, cancelled)
}

fn trash_file_with_before_move(
    file: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
    context: &TrashContext,
    before_move: impl FnOnce(&Path),
) -> Result<TrashedRecord, TrashError> {
    trash_file_checked(file, owning_root, content_hash, context, None, None, before_move, &|| false)
}

fn trash_file_checked(
    file: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
    context: &TrashContext,
    source_version: Option<crate::file_identity::FileVersion>,
    destination: Option<crate::file_identity::VerifiedDestination>,
    before_move: impl FnOnce(&Path),
    cancelled: &dyn Fn() -> bool,
) -> Result<TrashedRecord, TrashError> {
    if !file.is_absolute() {
        return Err(format!(
            "trash requires an absolute path: {}",
            crate::native_path::label(&file)
        )
        .into());
    }
    let (snapshot, metadata) = crate::file_identity::SourceSnapshot::open(file)
        .map_err(|error| format!("trash source is unavailable: {error}"))?;
    if source_version.is_some_and(|expected| expected != snapshot.version) {
        return Err(TrashError {
            source_changed: true,
            ..crate::file_identity::changed_source_error().to_string().into()
        });
    }
    let plan = prepare_trash(file, owning_root, content_hash, context, &metadata)?;
    commit_trash(file, plan, snapshot, destination, before_move, cancelled)
}

struct TrashPlan {
    record: TrashedRecord,
    stored: PathBuf,
    day_dir: PathBuf,
    private: crate::trash_privacy::Directory,
}

fn prepare_trash(
    original: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
    context: &TrashContext,
    metadata: &std::fs::Metadata,
) -> Result<TrashPlan, String> {
    if !owning_root.is_absolute() || !volume_io::is_dir(owning_root).unwrap_or(false) {
        return Err(format!(
            "deleted-file root is unavailable: {}",
            crate::native_path::label(&owning_root)
        ));
    }
    if !crate::path_identity::directory_is_within(original, owning_root)? {
        return Err(format!(
            "{} is outside its frozen configured root {}",
            crate::native_path::label(&original),
            crate::native_path::label(&owning_root)
        ));
    }
    // Compare physical volumes, not spellings: a root configured through a
    // mapped or `subst` drive or a symlink indexes its files under another
    // spelling of the same filesystem.
    let original_volume = crate::file_identity::volume_of(original)
        .map_err(|error| format!("trash source is unavailable: {error}"))?;
    let root_volume = crate::file_identity::volume_of(owning_root)
        .map_err(|error| format!("deleted-file root is unavailable: {error}"))?;
    if original_volume != root_volume {
        return Err("recoverable deletion must stay on the same filesystem".to_string());
    }
    let original_relative = relative_to_root(original, owning_root)?;
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
    let day_dir = trash_root.join(format!("{}{day}", crate::trash_privacy::account_prefix().map_err(|e| e.to_string())?));

    // FLAT: the day folder holds file names only, no preserved directory
    // structure. The manifest carries provenance, so the folder can be the
    // plain "everything deleted this day" view an OS trash shows — and a
    // trashed path never grows longer than <trash>/<day>/<name>, which is what
    // stopped the trash amplifying the platform's path-length limit.
    //
    let name = original
        .file_name()
        .ok_or_else(|| format!("{} has no file name", crate::native_path::label(&original)))?;
    let target = day_dir.join(name);
    // Deleted files stay inside the root's own permission boundary: an
    // existing `.onecopy-trash` or day folder must be a real directory there,
    // never a symlink or anything else that would lead elsewhere.
    ensure_real_directory_if_present(&trash_root)?;
    // Hide the trash root on Windows exactly once, the moment this call is
    // the one that creates it — never per trashed file.
    #[cfg_attr(not(windows), allow(unused_variables))]
    let trash_root_is_new = !volume_io::exists(&trash_root).map_err(|e| e.to_string())?;
    volume_io::create_dir_all(&trash_root).map_err(|e| e.to_string())?;
    let private = crate::trash_privacy::Directory::open(&day_dir, true).map_err(|e| e.to_string())?;
    ensure_real_directory_if_present(&trash_root)?;
    ensure_real_directory_if_present(&day_dir)?;
    if crate::file_identity::volume_of(&day_dir).map_err(|e| e.to_string())? != root_volume {
        return Err("recoverable deletion must stay on the same filesystem".to_string());
    }
    if !crate::path_identity::directory_is_within(&day_dir, owning_root)? {
        return Err("deleted-file storage escaped its configured root".to_string());
    }
    #[cfg(windows)]
    if trash_root_is_new {
        hide_windows(&trash_root);
    }

    let stored = available_stored_path(&target)?;

    let record = TrashedRecord {
        format_version: formats::DELETED_FILES_MANIFEST,
        original_path: crate::native_path::encode(&original).to_string(),
        stored_path: crate::native_path::encode(&stored).to_string(),
        content_hash: content_hash.map(|h| h.to_string()),
        deleted_at_utc: logging::now_iso_millis(),
        stored_name: stored
            .file_name()
            .map(|name| crate::native_path::encode(&name).into_owned())
            .unwrap_or_default(),
        original_relative,
        kind: context.kind,
        operation: context.operation.clone(),
        item: context.item.clone(),
        role: context.role,
        moved_to: context.moved_to.clone(),
        size: metadata.len(),
        mtime_ms: mtime_ms(metadata),
    };
    // Provenance commits FIRST. If append/fsync fails, the indexed source has
    // not moved and remains authoritative. A crash or an exact-boundary target
    // collision after this point can leave a harmless stale audit line, never
    // an untracked file whose original location was lost.
    let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
    refuse_newer_day(&day_dir)?;
    private.append(&day_dir.join(MANIFEST_FILE_NAME), line).map_err(|e| e.to_string())?;
    private.sync(&day_dir).map_err(|e| e.to_string())?;
    crate::fs_publish::sync_directory(&trash_root).map_err(|e| e.to_string())?;

    Ok(TrashPlan {
        record,
        stored,
        day_dir,
        private,
    })
}

fn commit_trash(
    source: &Path,
    plan: TrashPlan,
    snapshot: crate::file_identity::SourceSnapshot,
    destination: Option<crate::file_identity::VerifiedDestination>,
    before_move: impl FnOnce(&Path),
    cancelled: &dyn Fn() -> bool,
) -> Result<TrashedRecord, TrashError> {
    if cancelled() { return Err(TrashError { cancelled: true, .."cancelled".to_string().into() }); }
    before_move(&plan.stored);
    let moved = refuse_newer_day(&plan.day_dir)
        .map_err(|message| TrashError {
            message,
            outcome_unknown: false,
            source_changed: false,
            destination_changed: false,
            cancelled: false,
        })
        .and_then(|()| {
            #[cfg(target_os = "macos")]
            let moved = plan.private.move_source(source, &plan.stored, snapshot, destination);
            #[cfg(windows)]
            let moved = copy_before_remove(source, &plan, snapshot, destination, || {}, cancelled);
            #[cfg(not(any(target_os = "macos", windows)))]
            let moved: std::io::Result<()> = Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "private deletion is unsupported on this platform"));
            moved.map_err(|error| TrashError {
                message: format!("trash move failed for {}: {error}", crate::native_path::label(&source)),
                outcome_unknown: volume_io::outcome_unknown(&error),
                source_changed: crate::file_identity::source_version_changed(&error),
                destination_changed: crate::file_identity::destination_changed(&error),
                cancelled: error.kind() == std::io::ErrorKind::Interrupted && cancelled(),
            })
        });
    record_trash_action(&plan.record, moved.as_ref().err());
    moved?;
    if let Err(error) = plan.private.sync(&plan.day_dir) {
        crate::logging::warn(
            "trash directory sync failed after the move completed",
            json!({
                "path": crate::native_path::encode(&plan.day_dir),
                "error": { "message": error.to_string() },
            }),
        );
    }

    Ok(plan.record)
}

/// Shared orchestration is exercised on every host; Windows supplies the
/// protected file creator. Failures after publication retain the recovery
/// copy and the original, never roll recovery back after uncertain removal.
#[cfg(any(windows, test))]
fn copy_before_remove(
    source: &Path,
    plan: &TrashPlan,
    mut snapshot: crate::file_identity::SourceSnapshot,
    destination: Option<crate::file_identity::VerifiedDestination>,
    before_remove: impl FnOnce(),
    cancelled: &dyn Fn() -> bool,
) -> std::io::Result<()> {
    use crate::hashing::CopyFailure;
    snapshot.require_current(source)?;
    let expected = snapshot.version;
    let stage = plan.day_dir.join(crate::file_identity::private_stage_file_name().map_err(std::io::Error::other)?);
    let (mut copied, version) = crate::hashing::copy_to_private(source, &stage, || plan.private.create_file(&stage), cancelled)
        .map_err(|error| match error {
            CopyFailure::Source(error) | CopyFailure::Destination(error) => recovery_preparation_error(error),
            CopyFailure::Cancelled => std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"),
            CopyFailure::SourceChanged => crate::file_identity::changed_source_error(),
        })?;
    if expected != version {
        return Err(crate::file_identity::changed_source_error());
    }
    let writer = copied.file_mut();
    let metadata = writer.metadata()?;
    if metadata.len() != plan.record.size || mtime_ms(&metadata) != plan.record.mtime_ms {
        return Err(crate::file_identity::changed_source_error());
    }
    version.require_path(source)?;
    if cancelled() { return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")); }
    copied.publish(&plan.stored).map_err(recovery_preparation_error)?;
    plan.private.sync(&plan.day_dir).map_err(recovery_preparation_error)?;
    let stored_version = crate::file_identity::FileVersion::of(copied.file_mut())?;
    before_remove();
    if cancelled() { return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")); }
    let source_path = source.to_path_buf();
    #[cfg(not(windows))]
    let native = crate::winpath::for_fs(source).into_owned();
    let stored = plan.stored.clone();
    volume_io::call(source, volume_io::Op::Remove, None, move || {
        stored_version.require_path(&stored)?;
        snapshot.require_current(&source_path)?;
        if crate::file_identity::volume_of(&stored)? != crate::file_identity::volume_of(&source_path)? {
            return Err(std::io::Error::other("private recovery moved to a different filesystem"));
        }
        if let Some(destination) = &destination { destination.require_current()?; }
        #[cfg(windows)]
        { crate::trash_privacy::remove_original(&source_path, version) }
        #[cfg(not(windows))]
        { std::fs::remove_file(native) } // volume_io worker
    })
}

#[cfg(any(windows, test))]
fn recovery_preparation_error(error: std::io::Error) -> std::io::Error {
    // A timed-out write or publication may settle later, but no source
    // removal has even been submitted. Do not report its deletion as unknown.
    if volume_io::outcome_unknown(&error) {
        std::io::Error::new(error.kind(), format!("private recovery preparation failed; original retained: {error}"))
    } else { error }
}

fn record_trash_action(record: &TrashedRecord, failure: Option<&TrashError>) {
    let mut detail = json!(record);
    if let Some(failure) = failure {
        detail["error"] = json!({
            "message": failure.message,
            "outcomeUnknown": failure.outcome_unknown,
            "sourceChanged": failure.source_changed,
        });
    }
    crate::records::trash_action(crate::records::TrashAction {
        action: if failure.is_some() { "trash-failed" } else { "trashed" },
        operation_id: Some(&record.operation),
        content_hash: record.content_hash.as_deref(),
        original_path: Some(&record.original_path),
        stored_path: &record.stored_path,
        detail,
    });
}

/// A file's modification time in whole milliseconds since the Unix epoch,
/// the precision a manifest records and a reader compares.
pub(crate) fn mtime_ms(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| {
            match modified.duration_since(std::time::UNIX_EPOCH) {
                Ok(after) => i64::try_from(after.as_millis()).ok(),
                Err(before) => i64::try_from(before.duration().as_millis()).ok().map(|ms| -ms),
            }
        })
        .unwrap_or(0)
}

/// `file`'s path below `root` as `/`-joined components, resolved physically
/// so a root reached through a link or another spelling still yields the
/// path as it lies inside the root.
fn relative_to_root(file: &Path, root: &Path) -> Result<String, String> {
    let name = file
        .file_name()
        .ok_or_else(|| format!("{} has no file name", crate::native_path::label(&file)))?;
    let parent = file
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", crate::native_path::label(&file)))?;
    let resolved_parent = volume_io::canonicalize(parent)
        .map_err(|error| format!("trash source is unavailable: {error}"))?;
    let root_identity = volume_io::canonicalize(root)
        .and_then(|resolved| crate::file_identity::FileIdentity::from_path(&resolved))
        .map_err(|error| format!("deleted-file root is unavailable: {error}"))?;
    // The ancestor that IS the root, found by identity rather than spelling,
    // so a differently cased or linked spelling still finds it.
    let below = resolved_parent
        .ancestors()
        .find(|ancestor| {
            crate::file_identity::FileIdentity::from_path(ancestor)
                .is_ok_and(|identity| identity == root_identity)
        })
        .and_then(|ancestor| resolved_parent.strip_prefix(ancestor).ok())
        .ok_or_else(|| format!("{} is outside its frozen configured root {}", crate::native_path::label(&file), crate::native_path::label(&root)))?;
    let mut components: Vec<String> = below
        .components()
        .map(|component| crate::native_path::encode(component.as_os_str()).into_owned())
        .collect();
    components.push(crate::native_path::encode(&name).into_owned());
    Ok(components.join("/"))
}

fn ensure_real_directory_if_present(path: &Path) -> Result<(), String> {
    match volume_io::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "deleted-file storage is not a real directory: {}",
            crate::native_path::label(&path)
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("deleted-file storage is unavailable: {error}")),
    }
}

/// Selects `target`, falling back to `stem-2.ext`, `stem-3.ext`, … when the
/// name is occupied. Final authority is the later atomic exclusive rename: an
/// external exact-boundary winner is preserved and the source stays put.
fn available_stored_path(target: &Path) -> Result<PathBuf, String> {
    let mut candidate = target.to_path_buf();
    let mut counter = 2u32;
    loop {
        match volume_io::symlink_metadata(&candidate) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(err) => return Err(err.to_string()),
        }
        // A runaway guard, not a design limit: a hyphen and a number stay
        // short even at a million, and real collisions are rare.
        if counter > 1_000_000 {
            return Err(format!(
                "could not find a free trash name for {}",
                crate::native_path::label(&target)
            ));
        }
        candidate = suffixed_name(target, counter);
        counter += 1;
    }
}

/// `image1.jpg` + 2 → `image1-2.jpg`; extensionless names get `name-2`.
fn suffixed_name(target: &Path, counter: u32) -> PathBuf {
    let mut name = target
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new("file"))
        .to_os_string();
    name.push(format!("-{counter}"));
    if let Some(extension) = target.extension() {
        name.push(".");
        name.push(extension);
    }
    target.with_file_name(name)
}

fn append_manifest_line(day_dir: &Path, line: String) -> Result<(), String> {
    refuse_newer_day(day_dir)?;
    if crate::trash_privacy::is_private(day_dir) {
        crate::trash_privacy::Directory::open(day_dir, false)
            .and_then(|directory| directory.append(&day_dir.join(MANIFEST_FILE_NAME), line))
    } else {
        volume_io::append_line_synced(&day_dir.join(MANIFEST_FILE_NAME), line)
    }
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Reading manifests

/// One record as a reader sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayRecord {
    pub stored_name: String,
    /// The original path below the root as `/`-joined components.
    pub original_relative: String,
    pub deleted_at_utc: String,
    pub content_hash: Option<String>,
    pub kind: TrashKind,
    pub operation: String,
    pub item: Option<String>,
    pub role: TrashRole,
    pub moved_to: Option<String>,
    pub size: u64,
    pub mtime_ms: i64,
}

/// What the day folder holds under a record's stored name right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredState {
    Regular { size: u64, mtime_ms: i64 },
    /// A symlink, directory or other non-regular entry.
    Other,
}

/// One day folder as its manifest and its entries describe it.
#[derive(Debug, Default)]
pub struct DayListing {
    /// Authoritative records whose stored entry is present, in manifest
    /// order, each with what is stored under its name.
    pub records: Vec<(DayRecord, StoredState)>,
    /// Authoritative records whose stored file is gone and a `restored`
    /// line names after them.
    pub restored: u64,
    /// Those restored records with where the latest such line says the file
    /// went (`/`-joined below the root), when it says.
    pub restored_to: Vec<(DayRecord, String)>,
    /// Lines that are not a record or event this reader understands (a torn
    /// last line after a crash included). They are skipped, never repaired.
    pub malformed_lines: u64,
    /// Lines a newer OneCopy wrote. They are left as they are, unread.
    pub newer_lines: u64,
    /// Entries in the day folder that no authoritative record names.
    pub unrecorded_files: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoredLine {
    event: String,
    stored_name: String,
    restored_to: Option<String>,
}

enum ParsedLine {
    Record(DayRecord),
    Restored {
        stored_name: String,
        restored_to: Option<String>,
    },
    Newer,
}

/// A bare stored name: one component, never the manifest itself.
fn valid_stored_name(name: &str) -> bool {
    let Some(native) = crate::native_path::try_from_text(name) else {
        return false;
    };
    let mut components = native.components();
    !name.is_empty()
        && matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
        && native.file_name() == Some(native.as_os_str())
        && native.as_ref() != Path::new(MANIFEST_FILE_NAME)
}

/// One manifest line. A line without its format marker is unreadable, and a
/// line a newer OneCopy wrote is not interpreted.
fn parse_line(line: &str) -> Option<ParsedLine> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if formats::json_version(&value).ok()? > formats::DELETED_FILES_MANIFEST {
        return Some(ParsedLine::Newer);
    }
    if value.get("event").is_some() {
        let restored: RestoredLine = serde_json::from_value(value).ok()?;
        return (restored.event == "restored" && valid_stored_name(&restored.stored_name)).then_some(
            ParsedLine::Restored {
                stored_name: restored.stored_name,
                restored_to: restored.restored_to,
            },
        );
    }
    let record: DayRecord = serde_json::from_value(value).ok()?;
    valid_stored_name(&record.stored_name).then_some(ParsedLine::Record(record))
}

fn read_manifest(day_dir: &Path) -> Result<Vec<u8>, String> {
    match volume_io::read(&day_dir.join(MANIFEST_FILE_NAME)) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("could not read deleted-file records: {error}")),
    }
}

pub(crate) fn refuse_newer_day(day_dir: &Path) -> Result<(), String> {
    let manifest = read_manifest(day_dir)?;
    if String::from_utf8_lossy(&manifest).lines().any(|line| matches!(parse_line(line), Some(ParsedLine::Newer))) {
        return Err(format!(
            "{} contains deleted-file records written by a newer OneCopy and was left in place",
            crate::native_path::label(&day_dir),
        ));
    }
    Ok(())
}

/// Reads one day folder: its manifest, line by line, and its entries. For a
/// stored name the LATEST record naming it is authoritative: a name is only
/// handed out while it is free, so a later record always describes the file
/// now stored there, and an earlier one (a stale line whose move failed, or a
/// file restored since) describes nothing. Only entries directly inside this
/// day folder resolve. A missing manifest is an empty one.
pub fn read_day(day_dir: &Path) -> Result<DayListing, String> {
    let manifest = read_manifest(day_dir)?;
    let entries = volume_io::read_dir(day_dir, true)
        .map_err(|error| format!("could not list deleted files: {error}"))?;

    let mut listing = DayListing::default();
    // name -> (line index of the latest record, record)
    let mut latest: std::collections::HashMap<String, (usize, DayRecord)> =
        std::collections::HashMap::new();
    let mut restored_lines: Vec<(usize, String, Option<String>)> = Vec::new();
    for (index, line) in String::from_utf8_lossy(&manifest).lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line) {
            Some(ParsedLine::Record(record)) => {
                latest.insert(record.stored_name.clone(), (index, record));
            }
            Some(ParsedLine::Restored {
                stored_name,
                restored_to,
            }) => restored_lines.push((index, stored_name, restored_to)),
            Some(ParsedLine::Newer) => listing.newer_lines += 1,
            None => listing.malformed_lines += 1,
        }
    }
    if listing.newer_lines > 0 {
        latest.clear();
        restored_lines.clear();
    }

    let mut stored: std::collections::HashMap<String, StoredState> =
        std::collections::HashMap::new();
    for entry in entries {
        let name = crate::native_path::encode(&entry.file_name).into_owned();
        if name == MANIFEST_FILE_NAME {
            continue;
        }
        let state = match (entry.file_type, entry.metadata) {
            (Some(kind), Some(Ok(metadata))) if kind.is_file() => StoredState::Regular {
                size: metadata.len(),
                mtime_ms: mtime_ms(&metadata),
            },
            _ => StoredState::Other,
        };
        if latest.contains_key(&name) {
            stored.insert(name, state);
        } else {
            listing.unrecorded_files += 1;
        }
    }

    let mut records: Vec<(usize, DayRecord)> = latest.into_values().collect();
    records.sort_by_key(|(index, _)| *index);
    for (index, record) in records {
        match stored.get(&record.stored_name) {
            Some(state) => listing.records.push((record, *state)),
            None => {
                let mut after = restored_lines
                    .iter()
                    .filter(|(line, name, _)| *line > index && *name == record.stored_name)
                    .peekable();
                if after.peek().is_some() {
                    listing.restored += 1;
                    if let Some(to) = after.filter_map(|(_, _, to)| to.clone()).last() {
                        listing.restored_to.push((record, to));
                    }
                }
            }
        }
    }
    Ok(listing)
}

/// Appends a `restored` line for one stored file. Best-effort: the file is
/// already back in place, so a failure only means the record keeps no
/// trace of it, and is logged.
pub fn append_restored(day_dir: &Path, stored_name: &str, restored_to: &str) {
    let line = json!({
        "formatVersion": formats::DELETED_FILES_MANIFEST,
        "event": "restored",
        "storedName": stored_name,
        "restoredTo": restored_to,
        "restoredAtUtc": logging::now_iso_millis(),
    });
    crate::records::trash_action(crate::records::TrashAction {
        action: "restored",
        operation_id: None,
        content_hash: None,
        original_path: None,
        stored_path: &crate::native_path::encode(
            &day_dir.join(crate::native_path::from_text(stored_name)),
        ),
        detail: line.clone(),
    });
    if let Err(error) = append_manifest_line(day_dir, line.to_string())
    {
        crate::logging::warn(
            "restored record could not be written",
            json!({ "path": crate::native_path::encode(&day_dir), "storedName": stored_name, "error": { "message": error.to_string() } }),
        );
    }
}

// ---------------------------------------------------------------------------
// Browsing one root's deleted files

/// Whether a listed entry can be restored, and if not, why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryStatus {
    /// The stored file still has the recorded size and modification time.
    Restorable,
    /// The stored entry changed since it was deleted, or is not a regular
    /// file. Reveal only.
    Changed,
    /// The original name cannot be rebuilt on this system. Reveal only.
    Unrepresentable,
    /// The original location lies inside deleted-file storage or OneCopy's
    /// own data folder. Reveal only.
    Excluded,
}

impl EntryStatus {
    pub fn restorable(self) -> bool {
        matches!(self, Self::Restorable)
    }
}

/// One stored file with a record, as Deleted files lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    /// `<day folder>/<stored name>`: unique within one root's listing.
    pub id: String,
    pub day: String,
    pub stored_name: String,
    pub stored_path: String,
    /// The record's fields are `None` only for a selected file the listing
    /// no longer has (`restore::candidates`).
    pub original_relative: Option<String>,
    pub deleted_at_utc: String,
    /// The stored file's current size.
    pub size: u64,
    /// The modification time the record promises, which Restore checks again
    /// before moving the file.
    pub mtime_ms: Option<i64>,
    /// The deleted item this file belongs to: one operation's logical item,
    /// or for a file without one (a displaced destination file) the
    /// operation, folder and stem, which is the companion rule.
    pub group: String,
    pub kind: Option<TrashKind>,
    pub operation: Option<String>,
    pub item: Option<String>,
    pub role: Option<TrashRole>,
    pub moved_to: Option<String>,
    pub status: EntryStatus,
    /// For a companion: where a main file of its deleted item in its folder
    /// was restored to under another name (`/`-joined below the root), so it
    /// no longer pairs by name with a companion that comes back unchanged.
    pub main_restored_as: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashListing {
    pub entries: Vec<TrashEntry>,
    /// Stored files that no record names.
    pub unrecorded_files: u64,
    /// Manifest lines that could not be read.
    pub malformed_lines: u64,
    /// Manifest lines a newer OneCopy wrote, left as they are.
    pub newer_lines: u64,
}

/// Why a recorded original path cannot become a target inside its root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnfitPath {
    Unrepresentable,
    Excluded,
}

/// The components of a recorded root-relative path, when they can be placed
/// inside a root on this system: never empty, never `.` or `..`, never an
/// undecodable native unit, never through deleted-file storage, and on
/// Windows never a component Windows would read as a separator or a drive.
pub fn relative_components(relative: &str) -> Result<Vec<&str>, UnfitPath> {
    components_under(relative, cfg!(windows))
}

/// `relative_components` with Windows' name rules chosen by the caller, so
/// either system's rules can be proven on the other.
fn components_under(relative: &str, windows_names: bool) -> Result<Vec<&str>, UnfitPath> {
    let components: Vec<&str> = relative.split('/').collect();
    for component in &components {
        if crate::native_path::try_from_text(component).is_none()
            || component.is_empty() || *component == "." || *component == ".." {
            return Err(UnfitPath::Unrepresentable);
        }
        if windows_names && component.contains(['\\', ':']) {
            return Err(UnfitPath::Unrepresentable);
        }
    }
    if components
        .iter()
        .any(|component| component.eq_ignore_ascii_case(TRASH_DIR_NAME))
    {
        return Err(UnfitPath::Excluded);
    }
    Ok(components)
}

/// The target a recorded relative path names inside `root`.
pub fn target_in_root(root: &Path, relative: &str) -> Result<PathBuf, UnfitPath> {
    Ok(relative_components(relative)?
        .into_iter()
        .fold(root.to_path_buf(), |path, component| {
            path.join(crate::native_path::from_text(component))
        }))
}

/// Decides one record's status from the record and what is stored under its
/// name. Pure: the caller supplies `in_data_root` for the target.
pub fn entry_status(
    record: &DayRecord,
    stored: StoredState,
    root: &Path,
    data_root: &Path,
) -> EntryStatus {
    let StoredState::Regular { size, mtime_ms } = stored else {
        return EntryStatus::Changed;
    };
    if record.size != size || record.mtime_ms != mtime_ms {
        return EntryStatus::Changed;
    }
    match target_in_root(root, &record.original_relative) {
        Err(UnfitPath::Unrepresentable) => EntryStatus::Unrepresentable,
        Err(UnfitPath::Excluded) => EntryStatus::Excluded,
        Ok(target) if crate::paths::is_within_data_root(&target, data_root) => {
            EntryStatus::Excluded
        }
        Ok(_) => EntryStatus::Restorable,
    }
}

/// Which deleted item a record belongs to (`TrashEntry::group`).
fn group_key(record: &DayRecord) -> String {
    let operation = &record.operation;
    if let Some(item) = &record.item {
        return format!("item:{operation}:{item}");
    }
    let relative = record.original_relative.as_str();
    let (folder, name) = relative.rsplit_once('/').unwrap_or(("", relative));
    let native_name = crate::native_path::from_text(name);
    let stem = native_name
        .file_stem()
        .map(|stem| {
            crate::native_path::encode(&stem).nfc().collect::<String>().to_lowercase()
        })
        .unwrap_or_default();
    format!("stem:{operation}:{folder}/{stem}")
}

/// The configured root that owns a deleted-files location the interface
/// names (`<configured root>/.onecopy-trash`), from the current settings.
pub fn owning_root_of(configured_roots: &[PathBuf], location: &Path) -> Result<PathBuf, String> {
    configured_roots
        .iter()
        .find(|root| root.join(TRASH_DIR_NAME) == location)
        .cloned()
        .ok_or_else(|| "not a known deleted-files location".to_string())
}

/// Lists every stored file with a record beneath one configured root. The
/// location must be a real directory; a day folder replaced by a link is
/// refused, never followed. A root with no location yet lists nothing.
pub fn list_root(root: &Path, data_root: &Path) -> Result<TrashListing, String> {
    let location = root.join(TRASH_DIR_NAME);
    match volume_io::symlink_metadata(&location) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "deleted-file storage is not a real directory: {}",
                crate::native_path::label(&location)
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TrashListing::default())
        }
        Err(error) => return Err(format!("deleted-file storage is unavailable: {error}")),
    }
    let mut listing = TrashListing::default();
    // (deleted item, original folder) -> where a main file of it was restored
    // under another name.
    let mut restored_mains: std::collections::HashMap<(String, String), String> =
        std::collections::HashMap::new();
    let mut days = volume_io::read_dir(&location, false)
        .map_err(|error| format!("could not list deleted files: {error}"))?;
    days.sort_by(|left, right| left.file_name.cmp(&right.file_name));
    let account = crate::trash_privacy::account_prefix().map_err(|e| e.to_string())?;
    for day in days {
        if !crate::trash_privacy::visible_day(&day.file_name, &account) { continue; }
        match day.file_type {
            Some(kind) if kind.is_dir() => {}
            Some(kind) if kind.is_symlink() => {
                return Err(format!(
                    "deleted-file storage is not a real directory: {}",
                    crate::native_path::label(&day.path)
                ))
            }
            _ => continue,
        }
        let day_name = crate::native_path::encode(&day.file_name).into_owned();
        let read = read_day(&day.path)?;
        listing.unrecorded_files += read.unrecorded_files;
        listing.malformed_lines += read.malformed_lines;
        listing.newer_lines += read.newer_lines;
        for (record, restored_to) in &read.restored_to {
            let original = record.original_relative.as_str();
            if record.role == TrashRole::Main && restored_to != original {
                restored_mains.insert(
                    (group_key(record), folder_of(original).to_string()),
                    restored_to.clone(),
                );
            }
        }
        for (record, stored) in read.records {
            let status = entry_status(&record, stored, root, data_root);
            let group = group_key(&record);
            let size = match stored {
                StoredState::Regular { size, .. } => size,
                StoredState::Other => 0,
            };
            listing.entries.push(TrashEntry {
                id: format!("{day_name}/{}", record.stored_name),
                day: day_name.clone(),
                stored_path: crate::native_path::encode(
                    &day.path.join(crate::native_path::from_text(&record.stored_name)),
                ).into_owned(),
                stored_name: record.stored_name,
                original_relative: Some(record.original_relative),
                deleted_at_utc: record.deleted_at_utc,
                size,
                mtime_ms: Some(record.mtime_ms),
                group,
                kind: Some(record.kind),
                operation: Some(record.operation),
                item: record.item,
                role: Some(record.role),
                moved_to: record.moved_to,
                status,
                main_restored_as: None,
            });
        }
    }
    for entry in &mut listing.entries {
        if entry.role != Some(TrashRole::Companion) {
            continue;
        }
        let folder = entry.original_relative.as_deref().map(folder_of).unwrap_or_default();
        entry.main_restored_as = restored_mains
            .get(&(entry.group.clone(), folder.to_string()))
            .cloned();
    }
    Ok(listing)
}

/// The folder part of a `/`-joined relative path.
fn folder_of(relative: &str) -> &str {
    relative.rsplit_once('/').map_or("", |(folder, _)| folder)
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: the callback is a private
// exact-boundary seam, and the platform-chosen name rules a private helper;
// neither may widen the shipped Trash API.
#[path = "../tests/unit/trash.rs"]
mod boundary_tests;

/// Selects the most-specific configured root containing a planned file. A
/// nested root owns its own deleted files instead of leaking them into an
/// ancestor root with potentially broader permissions.
///
/// A file that is gone, or whose drive is away, is matched by spelling
/// against every form its root can take in the index: as configured, as the
/// filesystem spells it (Windows verbatim), and as the root resolves when it
/// is present. No move can occur until `trash_file` revalidates the live path,
/// so this only lets execution report such a file as one unavailable target.
pub fn root_for_file(file: &Path, configured_roots: &[PathBuf]) -> Result<PathBuf, String> {
    if !file.is_absolute() {
        return Err(format!("{} is not an absolute path", crate::native_path::label(&file)));
    }
    configured_roots
        .iter()
        .filter(|root| {
            if !root.is_absolute() {
                return false;
            }
            match crate::path_identity::directory_is_within(file, root) {
                Ok(within) => within,
                Err(_) => root_spellings(root)
                    .iter()
                    .any(|spelling| file.starts_with(spelling)),
            }
        })
        .max_by_key(|root| root.components().count())
        .cloned()
        .ok_or_else(|| format!("{} is outside every configured root", crate::native_path::label(&file)))
}

pub fn root_spellings(root: &Path) -> Vec<PathBuf> {
    let filesystem = crate::winpath::for_fs(root).into_owned();
    let mut spellings = vec![root.to_path_buf()];
    if let Ok(resolved) = volume_io::canonicalize(&filesystem) {
        spellings.push(resolved);
    }
    spellings.push(filesystem);
    spellings
}

/// One trash root's standing facts for the Trash surface: where it is, how
/// much it holds. Sizes are computed on demand — the surface opens rarely and
/// a cached number would only be a chance to lie.
#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrashRootInfo {
    pub root: String,
    /// Whether the configured root answers as a directory now. An
    /// unavailable root cannot be browsed or restored into.
    pub available: bool,
    pub bytes: u64,
    pub files: u64,
    /// Names exactly what these totals measured. Emptying requires it back
    /// and removes nothing when the location no longer matches.
    pub plan_token: String,
    /// Incomplete or unavailable totals provide no Empty authority. The
    /// configured root can still be available for browsing and a retry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurement_error: Option<String>,
    /// The root is in a synced folder (`cloud_files`): emptying it deletes
    /// the files from the cloud and other devices too.
    pub synced: bool,
}

/// Every configured source and destination root has one local deleted-files
/// directory. Missing directories report zero and are created only when the
/// user deletes a file or explicitly reveals that location.
pub fn overview(configured_roots: &[PathBuf]) -> Vec<TrashRootInfo> {
    let mut roots: Vec<(PathBuf, &PathBuf)> = Vec::new();
    for configured in configured_roots {
        let root = configured.join(TRASH_DIR_NAME);
        if !roots.iter().any(|(known, _)| *known == root) {
            roots.push((root, configured));
        }
    }
    let synced = crate::cloud_files::synced_folders();
    roots
        .into_iter()
        .map(|(root, configured)| {
            let (bytes, files, plan_token, measurement_error) = match measure_root(&root) {
                Ok(measure) => (measure.bytes, measure.files, measure.token, None),
                Err(error) => {
                    crate::logging::warn("trash totals could not be measured",
                        json!({ "path": crate::native_path::encode(&root), "error": { "message": error } }));
                    (0, 0, String::new(), Some(error))
                }
            };
            TrashRootInfo {
                synced: crate::cloud_files::is_synced(&root, &synced),
                root: crate::native_path::encode(&root).to_string(),
                available: volume_io::is_dir(configured).unwrap_or(false),
                bytes,
                files,
                plan_token,
                measurement_error,
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
    volume_io::create_dir_all(requested)
        .map_err(|error| format!("could not create deleted-files location: {error}"))?;
    let metadata = volume_io::symlink_metadata(requested)
        .map_err(|error| format!("deleted-files location is unavailable: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("deleted-files location is not a real directory".to_string());
    }
    if !crate::path_identity::directory_is_within(requested, owning_root)? {
        return Err("deleted-files location escaped its configured root".to_string());
    }
    Ok(requested.to_path_buf())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyProgress {
    pub done: u64,
    pub total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub failures: u64,
    /// Attempted removals that may still settle on their volume worker.
    pub unknown: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyOutcome {
    pub cancelled: bool,
    pub failures: u64,
    /// The location no longer holds what the confirmation measured, so
    /// nothing was removed and the user must review the new totals.
    pub plan_changed: bool,
}

/// Permanently removes one already-authorized trash root with progress over
/// recoverable files, but only while it still holds exactly what the reviewed
/// totals measured (`reviewed_token` from `overview`). Emptying is permanent
/// and confirms exact totals, so a location that gained or lost files since
/// then is left untouched and reported as changed. The caller holds the
/// mutation boundary, so OneCopy adds nothing to it while it empties. Manifests
/// do not inflate the overview totals, but remain authoritative Restore data
/// until every other entry was removed successfully. Partial or cancelled
/// Empty keeps them for surviving files. The root itself must remain a real
/// directory and inner symlinks are never followed. Cancellation is checked
/// while planning and between files; an individual filesystem deletion is
/// already atomic at that unit.
pub fn empty_root_with_progress(
    root: &Path,
    reviewed_token: &str,
    cancelled: &AtomicBool,
    progress: &dyn Fn(EmptyProgress),
    record_failure: &dyn Fn(&Path, &str) -> Result<(), String>,
) -> Result<EmptyOutcome, String> {
    let is_cancelled = || cancelled.load(Ordering::Relaxed);
    if is_cancelled() {
        return Ok(EmptyOutcome { cancelled: true, ..EmptyOutcome::default() });
    }
    match volume_io::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err("trash root is not a directory".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            progress(EmptyProgress::default());
            return Ok(EmptyOutcome::default());
        }
        Err(error) => return Err(error.to_string()),
    }
    let measure = match measure_root_with_cancel(root, Some(&is_cancelled)) {
        _ if is_cancelled() => return Ok(EmptyOutcome { cancelled: true, ..EmptyOutcome::default() }),
        measured => measured?,
    };
    if measure.token != reviewed_token {
        return Ok(EmptyOutcome {
            plan_changed: true,
            ..EmptyOutcome::default()
        });
    }
    let mut files = measure.entries;
    let mut directories = measure.directories;
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    // Never remove provenance before its stored files. On any failure, keep
    // all manifests: retaining an empty day's history is harmless, while a
    // surviving file without its record cannot be restored. A later complete
    // Empty removes these manifests through the same path.
    files.sort_by_key(|(path, _, _)| {
        path.file_name().is_some_and(|name| name == MANIFEST_FILE_NAME)
    });

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
                plan_changed: false,
            });
        }
        if snapshot.failures > 0
            && path.file_name().is_some_and(|name| name == MANIFEST_FILE_NAME)
        {
            continue;
        }
        let removed = volume_io::remove_file(&path);
        if recoverable {
            snapshot.done += 1;
            snapshot.bytes_done = snapshot.bytes_done.saturating_add(bytes);
            if let Err(error) = &removed {
                snapshot.failures += 1;
                snapshot.unknown += u64::from(volume_io::outcome_unknown(error));
            }
            // Physical outcomes survive a stopped volume or a subsequent
            // failure to record the Issue. Neither makes this file unstarted.
            progress(snapshot.clone());
        }
        if recoverable || removed.is_err() {
            crate::records::trash_action(crate::records::TrashAction {
                action: if removed.is_ok() { "removed" } else { "remove-failed" },
                operation_id: None,
                content_hash: None,
                original_path: None,
                stored_path: &crate::native_path::encode(&path),
                detail: json!({
                    "root": crate::native_path::encode(&root),
                    "bytes": bytes,
                    "error": removed.as_ref().err().map(|error| json!({ "message": error.to_string() })),
                }),
            });
        }
        if let Err(error) = removed {
            record_failure(&path, &error.to_string())?;
            crate::logging::warn(
                "trash entry removal failed",
                serde_json::json!({
                    "path": crate::native_path::encode(&path),
                    "error": { "message": error.to_string() },
                }),
            );
            // A volume that stopped answering ends the Empty here with its
            // partial totals; removal is idempotent, so emptying again is
            // safe once the drive answers.
            if volume_io::wait_failure(&error).is_some() || !recoverable {
                return Err(error.to_string());
            }
        }
    }
    for directory in directories {
        if let Err(error) = volume_io::remove_dir(&directory) {
            if !matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) {
                record_failure(&directory, &error.to_string())?;
                crate::logging::warn(
                    "trash directory cleanup failed",
                    serde_json::json!({
                        "path": crate::native_path::encode(&directory),
                        "error": { "message": error.to_string() },
                    }),
                );
            }
        }
    }
    Ok(EmptyOutcome {
        cancelled: false,
        failures: snapshot.failures,
        plan_changed: false,
    })
}

/// A complete, fresh snapshot of the entries Empty may remove. Folder mtime
/// cannot prove byte totals: editing a stored file or a nested child leaves
/// its day folder unchanged. Measure metadata on each overview and keep the
/// same measured paths for Empty, so a later-added entry is never unreviewed
/// work. No global cache lock can couple unrelated volumes.
fn measure_root(root: &Path) -> Result<TrashMeasure, String> {
    measure_root_with_cancel(root, None)
}

fn measure_root_with_cancel(
    root: &Path,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<TrashMeasure, String> {
    measure_root_at(root, root, cancelled)
}

/// Derive the entry prefix from the walk's filesystem spelling (verbatim on
/// Windows). Keep the walk input's original spelling for volume admission,
/// and the selected UI root for its configured ownership and identity.
fn measure_root_at(
    root: &Path,
    walk_root: &Path,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<TrashMeasure, String> {
    let filesystem_root = crate::winpath::for_fs(walk_root);
    let metadata = match volume_io::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => metadata,
        Ok(_) => return Err("trash root is not a real directory".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TrashMeasure {
                bytes: 0,
                files: 0,
                token: blake3::hash(b"absent").to_hex().to_string(),
                entries: Vec::new(),
                directories: Vec::new(),
            });
        }
        Err(error) => return Err(format!("trash root is unavailable: {error}")),
    };
    let root_identity = crate::file_identity::FileIdentity::from_path(root)
        .map_err(|error| format!("trash root identity is unavailable: {error}"))?;
    let root_modified = metadata.modified().map_err(|error| error.to_string())?;
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut entries = Vec::new();
    let mut directories = Vec::new();
    let mut versions = Vec::new();
    let account = crate::trash_privacy::account_prefix().map_err(|e| e.to_string())?;
    let filter_root = filesystem_root.into_owned();
    let mut walk = volume_io::walk(walk_root, move |path, _| {
        path.parent() != Some(filter_root.as_path())
            || path.file_name().is_none_or(|name| crate::trash_privacy::visible_day(name, &account))
    }).map_err(|error| error.to_string())?;
    let filesystem_root = crate::winpath::for_fs(walk_root);
    while let Some(entry) = walk.next(cancelled) {
        if cancelled.is_some_and(|cancelled| cancelled()) {
            return Err("trash totals measurement cancelled".to_string());
        }
        let entry = entry
            .map_err(|error| error.to_string())?
            .map_err(|error| error.message)?;
        // Read the entry itself: links are never followed, including a path
        // changed while the walk was queued. Every metadata failure makes
        // the entire review incomplete, never a smaller authorized total.
        let metadata = if entry.file_type.is_file() {
            entry
                .metadata
                .ok_or_else(|| "trash file metadata was not measured".to_string())?
                .map_err(|error| error.to_string())?
        } else {
            volume_io::symlink_metadata(&entry.path).map_err(|error| error.to_string())?
        };
        if metadata.file_type() != entry.file_type {
            return Err("trash entry changed while its totals were measured".to_string());
        }
        let relative = entry
            .path
            .strip_prefix(filesystem_root.as_ref())
            .map_err(|error| error.to_string())?
            .as_os_str()
            .to_os_string();
        let kind = if entry.file_type.is_dir() {
            b'd'
        } else if entry.file_type.is_file() {
            b'f'
        } else if entry.file_type.is_symlink() {
            b'l'
        } else {
            b'o'
        };
        versions.push((
            relative,
            kind,
            metadata.len(),
            metadata.modified().map_err(|error| error.to_string())?,
        ));
        if entry.depth == 0 {
            continue;
        }
        if entry.file_type.is_dir() {
            directories.push(entry.path);
        } else {
            let recoverable = entry.file_type.is_file()
                && entry
                    .path
                    .file_name()
                    .is_none_or(|name| name != MANIFEST_FILE_NAME);
            let size = if recoverable { metadata.len() } else { 0 };
            if recoverable {
                bytes += size;
                files += 1;
            }
            entries.push((entry.path, size, recoverable));
        }
    }
    let after = volume_io::symlink_metadata(root).map_err(|error| error.to_string())?;
    if !after.file_type().is_dir()
        || after.modified().map_err(|error| error.to_string())? != root_modified
        || crate::file_identity::FileIdentity::from_path(root).map_err(|error| error.to_string())?
            != root_identity
    {
        return Err("trash root changed while its totals were measured".to_string());
    }
    versions.sort();
    let mut hasher = blake3::Hasher::new();
    hasher.update(format!("{root_identity:?}").as_bytes());
    for (name, kind, size, modified) in versions {
        let name = name.as_encoded_bytes();
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name);
        hasher.update(&[kind]);
        hasher.update(&size.to_le_bytes());
        hasher.update(format!("{modified:?}").as_bytes());
    }
    Ok(TrashMeasure {
        bytes,
        files,
        token: hasher.finalize().to_hex().to_string(),
        entries,
        directories,
    })
}

struct TrashMeasure {
    bytes: u64,
    files: u64,
    token: String,
    entries: Vec<(PathBuf, u64, bool)>,
    directories: Vec<PathBuf>,
}

/// The volume (mount point / drive) root containing `path`.
#[cfg(unix)]
pub fn volume_root_of(path: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    let start = nearest_existing(path)?;
    let dev = volume_io::metadata(&start).map_err(|e| e.to_string())?.dev();
    let mut current = start;
    loop {
        let Some(parent) = current.parent() else {
            return Ok(current); // reached `/`
        };
        let parent_dev = volume_io::metadata(parent).map_err(|e| e.to_string())?.dev();
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
    let raw = crate::native_path::encode(&path);
    let conventional = crate::winpath::for_display(&raw);
    let native = crate::native_path::from_text(&conventional);
    let mut components = native.components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Prefix(prefix)), Some(std::path::Component::RootDir)) => {
            Ok(PathBuf::from(prefix.as_os_str()).join(std::path::MAIN_SEPARATOR.to_string()))
        }
        _ => Err(format!("no volume prefix in {}", crate::native_path::label(&path))),
    }
}

// Walks up to the nearest existing ancestor, so a just-deleted sibling or a
// not-yet-created leaf never breaks volume detection. Unix-only: the Windows
// `volume_root_of` reads the path prefix and never touches the filesystem.
#[cfg(unix)]
fn nearest_existing(path: &Path) -> Result<PathBuf, String> {
    let mut current = path.to_path_buf();
    while !volume_io::exists(&current).map_err(|error| error.to_string())? {
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }
    Ok(current)
}

#[cfg(windows)]
fn hide_windows(trash_root: &Path) {
    // Best-effort: mark the trash root hidden (dot-prefix means nothing to
    // Explorer). This calls the Win32 attribute API directly rather than
    // spawning `attrib`, so there is no console flash and no process start, in
    // one bounded call like every other step on the root's volume; it runs at
    // most once per trash root, when `prepare_trash` creates it.
    let wide: Vec<u16> = {
        use std::os::windows::ffi::OsStrExt;
        crate::winpath::for_fs(trash_root)
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect()
    };
    let hidden = volume_io::call(trash_root, volume_io::Op::Create, None, move || {
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN,
            INVALID_FILE_ATTRIBUTES,
        };
        // SAFETY: `wide` is an owned NUL-terminated path buffer alive for
        // both calls.
        let existing = unsafe { GetFileAttributesW(wide.as_ptr()) }; // volume_io worker
        let attrs = if existing == INVALID_FILE_ATTRIBUTES {
            FILE_ATTRIBUTE_HIDDEN
        } else {
            existing | FILE_ATTRIBUTE_HIDDEN
        };
        if unsafe { SetFileAttributesW(wide.as_ptr(), attrs) } == 0 { // volume_io worker
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    });
    if let Err(error) = hidden {
        crate::logging::warn(
            "trash directory could not be hidden",
            serde_json::json!({
                "path": crate::native_path::encode(&trash_root),
                "error": { "message": error.to_string() },
            }),
        );
    }
}

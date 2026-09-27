//! Recoverable deleted-file storage: root-local, day-foldered, and
//! append-only (the app never purges or rewrites it; deleting any day folder
//! by hand is safe because nothing outside it references its contents — the
//! invariant the design states). This module owns the manifest format, its
//! writer and its reader (`read_day`), which Restore builds on.
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

use std::path::{Path, PathBuf};

use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

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

/// The current manifest line format. Lines without `v` are the original
/// four-field format and stay readable (`read_day`).
pub const MANIFEST_VERSION: u32 = 2;

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

/// One manifest line (version 2). The four original fields stay first and
/// unchanged, so every older reader and line keeps its meaning; the rest are
/// additive. `storedName` and `originalRelative` are relative, so a record
/// still resolves after the root is renamed or its drive is mounted
/// elsewhere; `size` and `mtimeMs` let a reader prove the stored file is the
/// one the record describes.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrashedRecord {
    pub original_path: String,
    pub stored_path: String,
    pub content_hash: Option<String>,
    pub deleted_at_utc: String,
    pub v: u32,
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
}

impl From<String> for TrashError {
    fn from(message: String) -> Self {
        Self {
            message,
            outcome_unknown: false,
        }
    }
}

impl std::fmt::Display for TrashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Moves one file beneath the frozen configured root that owns it.
pub fn trash_file(
    file: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
    context: &TrashContext,
) -> Result<TrashedRecord, TrashError> {
    trash_file_with_before_move(file, owning_root, content_hash, context, |_| {})
}

fn trash_file_with_before_move(
    file: &Path,
    owning_root: &Path,
    content_hash: Option<&str>,
    context: &TrashContext,
    before_move: impl FnOnce(&Path),
) -> Result<TrashedRecord, TrashError> {
    if !file.is_absolute() {
        return Err(format!(
            "trash requires an absolute path: {}",
            file.display()
        )
        .into());
    }
    let metadata = volume_io::symlink_metadata(file)
        .map_err(|error| format!("trash source is unavailable: {error}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "trash source is not a regular file: {}",
            file.display()
        )
        .into());
    }
    let plan = prepare_trash(file, owning_root, content_hash, context, &metadata)?;
    commit_trash(file, plan, before_move)
}

struct TrashPlan {
    record: TrashedRecord,
    stored: PathBuf,
    day_dir: PathBuf,
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
    // Deleted files stay inside the root's own permission boundary: an
    // existing `.onecopy-trash` or day folder must be a real directory there,
    // never a symlink or anything else that would lead elsewhere.
    ensure_real_directory_if_present(&trash_root)?;
    // Hide the trash root on Windows exactly once, the moment this call is
    // the one that creates it — never per trashed file (R1-12, R6-04).
    #[cfg_attr(not(windows), allow(unused_variables))]
    let trash_root_is_new = !volume_io::exists(&trash_root).map_err(|e| e.to_string())?;
    volume_io::create_dir_all(&day_dir).map_err(|e| e.to_string())?;
    ensure_real_directory_if_present(&trash_root)?;
    ensure_real_directory_if_present(&day_dir)?;
    if !crate::path_identity::directory_is_within(&day_dir, owning_root)? {
        return Err("deleted-file storage escaped its configured root".to_string());
    }
    #[cfg(windows)]
    if trash_root_is_new {
        hide_windows(&trash_root);
    }

    let stored = available_stored_path(&target)?;

    let record = TrashedRecord {
        original_path: original.to_string_lossy().to_string(),
        stored_path: stored.to_string_lossy().to_string(),
        content_hash: content_hash.map(|h| h.to_string()),
        deleted_at_utc: logging::now_iso_millis(),
        v: MANIFEST_VERSION,
        stored_name: stored
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
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
    append_manifest(&day_dir, &record)?;
    crate::fs_publish::sync_directory(&day_dir).map_err(|e| e.to_string())?;

    Ok(TrashPlan {
        record,
        stored,
        day_dir,
    })
}

fn commit_trash(
    source: &Path,
    plan: TrashPlan,
    before_move: impl FnOnce(&Path),
) -> Result<TrashedRecord, TrashError> {
    before_move(&plan.stored);
    crate::fs_publish::rename_no_replace(source, &plan.stored).map_err(|error| TrashError {
        message: format!("trash move failed for {}: {error}", source.display()),
        outcome_unknown: volume_io::outcome_unknown(&error),
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

    Ok(plan.record)
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
        .ok_or_else(|| format!("{} has no file name", file.display()))?;
    let parent = file
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", file.display()))?;
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
        .ok_or_else(|| format!("{} is outside its frozen configured root {}", file.display(), root.display()))?;
    let mut components: Vec<String> = below
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    components.push(name.to_string_lossy().into_owned());
    Ok(components.join("/"))
}

fn ensure_real_directory_if_present(path: &Path) -> Result<(), String> {
    match volume_io::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "deleted-file storage is not a real directory: {}",
            path.display()
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
    // not recorded: the manifest is trash-side audit data, append-mode by
    // construction, never managed text.
    volume_io::append_synced(
        &day_dir.join(MANIFEST_FILE_NAME),
        format!("{line}\n").into_bytes(),
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Reading manifests

/// One record as a reader sees it: a version 2 line, or an original
/// four-field line with what can be derived from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayRecord {
    /// 1 for an original four-field line, otherwise the line's `v`.
    pub version: u32,
    pub stored_name: String,
    /// The original path below the root as `/`-joined components; `None`
    /// when an original line's absolute path does not start with any
    /// spelling of the current root.
    pub original_relative: Option<String>,
    pub deleted_at_utc: String,
    pub content_hash: Option<String>,
    pub kind: Option<TrashKind>,
    pub operation: Option<String>,
    pub item: Option<String>,
    pub role: Option<TrashRole>,
    pub moved_to: Option<String>,
    pub size: Option<u64>,
    pub mtime_ms: Option<i64>,
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
    /// Lines that are not a record or event this reader understands (a torn
    /// last line after a crash included). They are skipped, never repaired.
    pub malformed_lines: u64,
    /// Entries in the day folder that no authoritative record names.
    pub unrecorded_files: u64,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawLine {
    v: Option<u32>,
    event: Option<String>,
    original_path: Option<String>,
    stored_path: Option<String>,
    content_hash: Option<String>,
    deleted_at_utc: Option<String>,
    stored_name: Option<String>,
    original_relative: Option<String>,
    kind: Option<TrashKind>,
    operation: Option<String>,
    item: Option<String>,
    role: Option<TrashRole>,
    moved_to: Option<String>,
    size: Option<u64>,
    mtime_ms: Option<i64>,
}

enum ParsedLine {
    Record(DayRecord),
    Restored(String),
}

/// A bare stored name: one component, never the manifest itself.
fn valid_stored_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name != MANIFEST_FILE_NAME
        && !name.contains(['/', '\\', '\0'])
}

/// The stored name of an original line, only when its stored path is a flat
/// entry of this very day folder (`…/.onecopy-trash/<day>/<name>`). Nested
/// layouts and paths into other folders never resolve here. Both separators
/// are accepted, because the drive may have been written on the other OS.
fn legacy_stored_name(stored_path: &str, day: &str) -> Option<String> {
    let mut parts = stored_path.rsplit(['/', '\\']);
    let name = parts.next()?;
    let folder = parts.next()?;
    let trash = parts.next()?;
    (folder == day && trash.eq_ignore_ascii_case(TRASH_DIR_NAME) && valid_stored_name(name))
        .then(|| name.to_string())
}

/// An original line's absolute path below one of the root's spellings.
fn legacy_relative(original_path: &str, root_spellings: &[PathBuf]) -> Option<String> {
    let original = Path::new(original_path);
    root_spellings.iter().find_map(|spelling| {
        let below = original.strip_prefix(spelling).ok()?;
        let components = below
            .components()
            .map(|component| match component {
                std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        (!components.is_empty()).then(|| components.join("/"))
    })
}

fn parse_line(line: &str, day: &str, root_spellings: &[PathBuf]) -> Option<ParsedLine> {
    let raw: RawLine = serde_json::from_str(line).ok()?;
    if let Some(event) = raw.event.as_deref() {
        return (event == "restored")
            .then_some(raw.stored_name)
            .flatten()
            .filter(|name| valid_stored_name(name))
            .map(ParsedLine::Restored);
    }
    let deleted_at_utc = raw.deleted_at_utc?;
    match raw.v {
        None => {
            let stored_name = legacy_stored_name(raw.stored_path.as_deref()?, day)?;
            let original_relative = legacy_relative(raw.original_path.as_deref()?, root_spellings);
            Some(ParsedLine::Record(DayRecord {
                version: 1,
                stored_name,
                original_relative,
                deleted_at_utc,
                content_hash: raw.content_hash,
                kind: None,
                operation: None,
                item: None,
                role: None,
                moved_to: None,
                size: None,
                mtime_ms: None,
            }))
        }
        Some(version) if version >= 2 => {
            let stored_name = raw.stored_name.filter(|name| valid_stored_name(name))?;
            Some(ParsedLine::Record(DayRecord {
                version,
                stored_name,
                original_relative: Some(raw.original_relative?),
                deleted_at_utc,
                content_hash: raw.content_hash,
                kind: raw.kind,
                operation: raw.operation,
                item: raw.item,
                role: raw.role,
                moved_to: raw.moved_to,
                size: Some(raw.size?),
                mtime_ms: Some(raw.mtime_ms?),
            }))
        }
        Some(_) => None,
    }
}

/// Reads one day folder: its manifest, line by line, and its entries. For a
/// stored name the LATEST record naming it is authoritative: a name is only
/// handed out while it is free, so a later record always describes the file
/// now stored there, and an earlier one (a stale line whose move failed, or a
/// file restored since) describes nothing. Only entries directly inside this
/// day folder resolve. A missing manifest is an empty one.
pub fn read_day(day_dir: &Path, root_spellings: &[PathBuf]) -> Result<DayListing, String> {
    let day = day_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let manifest = match volume_io::read(&day_dir.join(MANIFEST_FILE_NAME)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("could not read deleted-file records: {error}")),
    };
    let entries = volume_io::read_dir(day_dir, true)
        .map_err(|error| format!("could not list deleted files: {error}"))?;

    let mut listing = DayListing::default();
    // name -> (line index of the latest record, record)
    let mut latest: std::collections::HashMap<String, (usize, DayRecord)> =
        std::collections::HashMap::new();
    let mut restored_lines: Vec<(usize, String)> = Vec::new();
    for (index, line) in String::from_utf8_lossy(&manifest).lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line, &day, root_spellings) {
            Some(ParsedLine::Record(record)) => {
                latest.insert(record.stored_name.clone(), (index, record));
            }
            Some(ParsedLine::Restored(name)) => restored_lines.push((index, name)),
            None => listing.malformed_lines += 1,
        }
    }

    let mut stored: std::collections::HashMap<String, StoredState> =
        std::collections::HashMap::new();
    for entry in entries {
        let name = entry.file_name.to_string_lossy().into_owned();
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
                if restored_lines
                    .iter()
                    .any(|(line, name)| *line > index && *name == record.stored_name)
                {
                    listing.restored += 1;
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
        "v": MANIFEST_VERSION,
        "event": "restored",
        "storedName": stored_name,
        "restoredTo": restored_to,
        "restoredAtUtc": logging::now_iso_millis(),
    });
    if let Err(error) = volume_io::append_synced(
        &day_dir.join(MANIFEST_FILE_NAME),
        format!("{line}\n").into_bytes(),
    ) {
        crate::logging::warn(
            "restored record could not be written",
            json!({ "path": day_dir, "storedName": stored_name, "error": { "message": error.to_string() } }),
        );
    }
}

// ---------------------------------------------------------------------------
// Browsing one root's deleted files

/// Whether a listed entry can be restored, and if not, why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryStatus {
    /// A version 2 record whose stored file still has the recorded size and
    /// modification time.
    Restorable,
    /// An original four-field record: restorable, but nothing proves the
    /// stored file is the one it describes.
    Unverified,
    /// The stored entry changed since it was deleted, or is not a regular
    /// file. Reveal only.
    Changed,
    /// The original location is not inside this root. Reveal only.
    OutsideRoot,
    /// The original name cannot be rebuilt on this system. Reveal only.
    Unrepresentable,
    /// The original location lies inside deleted-file storage or OneCopy's
    /// own data folder. Reveal only.
    Excluded,
}

impl EntryStatus {
    pub fn restorable(self) -> bool {
        matches!(self, Self::Restorable | Self::Unverified)
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
    pub original_relative: Option<String>,
    pub deleted_at_utc: String,
    /// The stored file's current size.
    pub size: u64,
    pub version: u32,
    pub kind: Option<TrashKind>,
    pub operation: Option<String>,
    pub item: Option<String>,
    pub role: Option<TrashRole>,
    pub moved_to: Option<String>,
    pub status: EntryStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashListing {
    pub entries: Vec<TrashEntry>,
    /// Stored files that no record names.
    pub unrecorded_files: u64,
    /// Manifest lines that could not be read.
    pub malformed_lines: u64,
}

/// Why a recorded original path cannot become a target inside its root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnfitPath {
    Unrepresentable,
    Excluded,
}

/// The components of a recorded root-relative path, when they can be placed
/// inside a root on this system: never empty, never `.` or `..`, never a
/// lossy replacement character, never through deleted-file storage, and on
/// Windows never a component Windows would read as a separator or a drive.
pub fn relative_components(relative: &str) -> Result<Vec<&str>, UnfitPath> {
    if relative.contains('\u{FFFD}') || relative.contains('\0') {
        return Err(UnfitPath::Unrepresentable);
    }
    let components: Vec<&str> = relative.split('/').collect();
    for component in &components {
        if component.is_empty() || *component == "." || *component == ".." {
            return Err(UnfitPath::Unrepresentable);
        }
        if cfg!(windows) && component.contains(['\\', ':']) {
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
        .fold(root.to_path_buf(), |path, component| path.join(component)))
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
    if record.version >= 2 && (record.size != Some(size) || record.mtime_ms != Some(mtime_ms)) {
        return EntryStatus::Changed;
    }
    let Some(relative) = record.original_relative.as_deref() else {
        return EntryStatus::OutsideRoot;
    };
    match target_in_root(root, relative) {
        Err(UnfitPath::Unrepresentable) => EntryStatus::Unrepresentable,
        Err(UnfitPath::Excluded) => EntryStatus::Excluded,
        Ok(target) if crate::paths::is_within_data_root(&target, data_root) => {
            EntryStatus::Excluded
        }
        Ok(_) if record.version >= 2 => EntryStatus::Restorable,
        Ok(_) => EntryStatus::Unverified,
    }
}

fn is_companion_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            crate::extensions::COMPANION_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
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
                location.display()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TrashListing::default())
        }
        Err(error) => return Err(format!("deleted-file storage is unavailable: {error}")),
    }
    let spellings = root_spellings(root);
    let mut listing = TrashListing::default();
    let mut days = volume_io::read_dir(&location, false)
        .map_err(|error| format!("could not list deleted files: {error}"))?;
    days.sort_by(|left, right| left.file_name.cmp(&right.file_name));
    for day in days {
        match day.file_type {
            Some(kind) if kind.is_dir() => {}
            Some(kind) if kind.is_symlink() => {
                return Err(format!(
                    "deleted-file storage is not a real directory: {}",
                    day.path.display()
                ))
            }
            _ => continue,
        }
        let day_name = day.file_name.to_string_lossy().into_owned();
        let read = read_day(&day.path, &spellings)?;
        listing.unrecorded_files += read.unrecorded_files;
        listing.malformed_lines += read.malformed_lines;
        for (record, stored) in read.records {
            let status = entry_status(&record, stored, root, data_root);
            let companion = is_companion_name(&record.stored_name);
            let size = match stored {
                StoredState::Regular { size, .. } => size,
                StoredState::Other => 0,
            };
            listing.entries.push(TrashEntry {
                id: format!("{day_name}/{}", record.stored_name),
                day: day_name.clone(),
                stored_path: day.path.join(&record.stored_name).to_string_lossy().into_owned(),
                stored_name: record.stored_name,
                original_relative: record.original_relative,
                deleted_at_utc: record.deleted_at_utc,
                size,
                version: record.version,
                kind: record.kind,
                operation: record.operation,
                item: record.item,
                // An older record has no role; the file's extension tells a
                // companion the way the library tells one.
                role: record.role.or_else(|| {
                    Some(if companion {
                        TrashRole::Companion
                    } else {
                        TrashRole::Main
                    })
                }),
                moved_to: record.moved_to,
                status,
            });
        }
    }
    Ok(listing)
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: the callback is a private
// exact-boundary seam and must not widen the shipped Trash API.
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
                Err(_) => root_spellings(root)
                    .iter()
                    .any(|spelling| file.starts_with(spelling)),
            }
        })
        .max_by_key(|root| root.components().count())
        .cloned()
        .ok_or_else(|| format!("{} is outside every configured root", file.display()))
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
    roots
        .into_iter()
        .map(|(root, configured)| {
            let measure = measure_root(&root);
            TrashRootInfo {
                root: root.to_string_lossy().to_string(),
                available: volume_io::is_dir(configured).unwrap_or(false),
                bytes: measure.bytes,
                files: measure.files,
                plan_token: measure.token,
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
/// mutation boundary, so OneCopy adds nothing to it while it empties. Manifests are bookkeeping and do not inflate the same
/// totals the overview/confirmation shows. The root itself must remain a real
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
    match volume_io::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err("trash root is not a directory".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            progress(EmptyProgress::default());
            return Ok(EmptyOutcome::default());
        }
        Err(error) => return Err(error.to_string()),
    }
    if measure_root(root).token != reviewed_token {
        return Ok(EmptyOutcome {
            plan_changed: true,
            ..EmptyOutcome::default()
        });
    }

    let mut files: Vec<(PathBuf, u64, bool)> = Vec::new();
    let mut directories: Vec<PathBuf> = Vec::new();
    let is_cancelled = || cancelled.load(Ordering::Relaxed);
    let mut walk = volume_io::walk(root, |_, _| true).map_err(|error| error.to_string())?;
    while let Some(entry) = walk.next(Some(&is_cancelled)) {
        if is_cancelled() {
            return Ok(EmptyOutcome {
                cancelled: true,
                ..EmptyOutcome::default()
            });
        }
        let entry = entry
            .map_err(|error| error.to_string())?
            .map_err(|error| error.message)?;
        if entry.depth == 0 {
            continue;
        }
        if entry.file_type.is_dir() {
            directories.push(entry.path);
        } else {
            // Symlinks and other stray non-directories are bookkeeping, never
            // followed and never counted as recoverable media, but Empty must
            // still remove their directory entries.
            let recoverable = entry.file_type.is_file()
                && entry.path.file_name().is_none_or(|name| name != MANIFEST_FILE_NAME);
            let bytes = recoverable
                .then(|| {
                    entry
                        .metadata
                        .and_then(Result::ok)
                        .map_or(0, |metadata| metadata.len())
                })
                .unwrap_or(0);
            files.push((entry.path, bytes, recoverable));
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
                plan_changed: false,
            });
        }
        if let Err(error) = volume_io::remove_file(&path) {
            snapshot.failures += 1;
            record_failure(&path, &error.to_string())?;
            crate::logging::warn(
                "trash entry removal failed",
                serde_json::json!({
                    "path": path,
                    "error": { "message": error.to_string() },
                }),
            );
            // A volume that stopped answering ends the Empty here with its
            // partial totals; removal is idempotent, so emptying again is
            // safe once the drive answers.
            if volume_io::wait_failure(&error).is_some() {
                return Err(error.to_string());
            }
        }
        if recoverable {
            snapshot.done += 1;
            snapshot.bytes_done = snapshot.bytes_done.saturating_add(bytes);
            progress(snapshot.clone());
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
        plan_changed: false,
    })
}

struct CachedDaySize {
    modified: std::time::SystemTime,
    /// When `bytes` and `files` were counted.
    measured: std::time::SystemTime,
    bytes: u64,
    files: u64,
}

/// The coarsest directory mtime granularity OneCopy meets (FAT's two
/// seconds). A change within this window after a count can leave a day
/// folder's mtime equal to the one the count saw.
const MTIME_GRANULARITY: std::time::Duration = std::time::Duration::from_secs(2);

impl CachedDaySize {
    /// Whether this count still describes a day folder whose mtime is
    /// `modified`. An equal mtime proves nothing changed only when the count
    /// was taken more than one mtime granule after that mtime; otherwise a
    /// file added in the same granule would leave the mtime unchanged.
    fn describes(&self, modified: std::time::SystemTime) -> bool {
        self.modified == modified
            && self
                .measured
                .duration_since(modified)
                .is_ok_and(|settled| settled > MTIME_GRANULARITY)
    }
}

/// Per-day-folder size cache, keyed by that folder's own path. Trash layout is
/// exactly `<root>/.onecopy-trash/<day>/<stored file or manifest>` (one level
/// of files beneath one level of day folders), so a day folder's own mtime
/// changes on every add, remove, or rename directly inside it -- whether done
/// by OneCopy or, per this app's recovery contract, by hand outside it. That
/// makes the day folder the exact right cache boundary: reusing its cached
/// size whenever its mtime is unchanged, once that mtime is older than one
/// mtime granule at counting time (`CachedDaySize::describes`), never risks
/// serving a stale total,
/// while a full walk of every day folder on every Trash-modal open (C-L3) is
/// avoided for the common case of reopening it with nothing changed.
static DAY_SIZE_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, CachedDaySize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Total bytes and file count of the RECOVERABLE contents of a tree; a missing
/// tree is (0, 0).
///
/// The per-day `manifest.jsonl` is excluded. It is our own bookkeeping, not
/// something the user put in the trash, and counting it made the overview
/// disagree with itself: a trash holding two deleted photos read "3 files",
/// and a trash emptied of everything recoverable could still read "1 file" —
/// with no way to reach zero. The count answers "how much of my library is in
/// here", so only entries a restore could hand back may contribute.
///
/// The token covers every entry of the root and each day folder's own mtime
/// and totals: any file added to or removed from a day folder changes that
/// folder's mtime, so an unchanged token means the root still holds what
/// these totals measured.
fn measure_root(root: &Path) -> TrashMeasure {
    // A trash root is created lazily by the first delete. Until then its
    // absence is the ordinary empty state promised by `overview`, not a walk
    // failure worth surfacing in the application log.
    let Ok(day_entries) = volume_io::read_dir(root, true) else {
        return TrashMeasure {
            bytes: 0,
            files: 0,
            token: blake3::hash(b"absent").to_hex().to_string(),
        };
    };
    let mut token_parts: Vec<(std::ffi::OsString, Option<(u128, u64, u64)>)> = Vec::new();

    let mut cache = DAY_SIZE_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut total_bytes = 0u64;
    let mut total_files = 0u64;
    let mut seen = std::collections::HashSet::new();
    for entry in day_entries {
        if !entry.file_type.is_some_and(|kind| kind.is_dir()) {
            token_parts.push((entry.file_name, None));
            continue;
        }
        let day_path = entry.path;
        let modified = match entry
            .metadata
            .unwrap_or_else(|| Err(std::io::Error::other("no metadata listed")))
            .and_then(|metadata| metadata.modified())
        {
            Ok(modified) => modified,
            Err(error) => {
                crate::logging::warn(
                    "trash day metadata read failed",
                    json!({ "path": day_path, "error": { "message": error.to_string() } }),
                );
                token_parts.push((entry.file_name, None));
                continue;
            }
        };
        let modified_nanos = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        seen.insert(day_path.clone());
        if let Some(cached) = cache.get(&day_path) {
            if cached.describes(modified) {
                total_bytes += cached.bytes;
                total_files += cached.files;
                token_parts.push((
                    entry.file_name.clone(),
                    Some((modified_nanos, cached.bytes, cached.files)),
                ));
                continue;
            }
        }
        let measured = std::time::SystemTime::now();
        let (bytes, files) = day_dir_size(&day_path);
        total_bytes += bytes;
        total_files += files;
        token_parts.push((entry.file_name, Some((modified_nanos, bytes, files))));
        cache.insert(
            day_path,
            CachedDaySize {
                modified,
                measured,
                bytes,
                files,
            },
        );
    }
    // Drop cache entries for day folders this root no longer has (emptied or
    // manually removed), without disturbing other roots' cached entries.
    cache.retain(|path, _| !path.starts_with(root) || seen.contains(path));
    token_parts.sort();
    let mut hasher = blake3::Hasher::new();
    for (name, day) in &token_parts {
        let name = name.as_encoded_bytes();
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name);
        match day {
            Some((modified, bytes, files)) => {
                hasher.update(b"d");
                hasher.update(&modified.to_le_bytes());
                hasher.update(&bytes.to_le_bytes());
                hasher.update(&files.to_le_bytes());
            }
            None => {
                hasher.update(b"o");
            }
        }
    }
    TrashMeasure {
        bytes: total_bytes,
        files: total_files,
        token: hasher.finalize().to_hex().to_string(),
    }
}

struct TrashMeasure {
    bytes: u64,
    files: u64,
    token: String,
}

/// Walks one day folder's own contents. Bounded to a single day's files
/// rather than the whole trash tree, and defensively recursive in case a
/// future stored layout ever nests beneath the day folder.
fn day_dir_size(day_dir: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut walk = match volume_io::walk(day_dir, |_, _| true) {
        Ok(walk) => walk,
        Err(error) => {
            crate::logging::warn(
                "trash size walk failed",
                json!({ "path": day_dir, "error": { "message": error.to_string() } }),
            );
            return (bytes, files);
        }
    };
    while let Some(entry) = walk.next(None) {
        let entry = match entry {
            Ok(Ok(entry)) => entry,
            Ok(Err(error)) => {
                crate::logging::warn(
                    "trash size walk failed",
                    json!({ "path": day_dir, "error": { "message": error.message } }),
                );
                continue;
            }
            // Given up on: the volume stopped answering; count what was seen.
            Err(_) => break,
        };
        if entry.file_type.is_file()
            && entry.path.file_name().is_none_or(|name| name != MANIFEST_FILE_NAME)
        {
            files += 1;
            match entry.metadata {
                Some(Ok(metadata)) => bytes += metadata.len(),
                Some(Err(error)) => crate::logging::warn(
                    "trash file metadata read failed",
                    json!({ "path": entry.path, "error": { "message": error.to_string() } }),
                ),
                None => {}
            }
        }
    }
    (bytes, files)
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
    // spawning `attrib`, so there is no console flash and no process start
    // (R1-12, R6-04), in one bounded call like every other step on the root's
    // volume; it runs at most once per trash root, when `prepare_trash`
    // creates it.
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
                "path": trash_root,
                "error": { "message": error.to_string() },
            }),
        );
    }
}

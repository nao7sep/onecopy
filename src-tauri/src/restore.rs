//! Restore: moving chosen stored files from one configured root's deleted
//! files back to their original paths inside that same root.
//!
//! The plan is decided by a pure planner (`plan_restore`) over what the edge
//! observed (`observe`), frozen by a token when a review is needed, and
//! executed file by file (`execute`). Every step is a same-volume,
//! no-replace rename: bytes are never copied and nothing is ever replaced.
//! Restored files in a source root are re-read into the index here, while the
//! caller still holds the index claim; nothing is seeded from the record.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use crate::file_names::{self, FolderNames, RenameStyle};
use crate::logging;
use crate::trash::{self, EntryStatus, TrashEntry, TrashRole};
use crate::volume_io;

/// Issue kind for a stored file Restore could not bring back.
pub const RESTORE_ERROR: &str = "restore-error";
/// Issue kind for a restore rename given up on while its drive was not
/// responding: the file is either still in Deleted files or at its target.
pub const RESTORE_OUTCOME_UNKNOWN: &str = "restore-outcome-unknown";

/// Why a selected file is not restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Skip {
    /// The same bytes are already at the original path. Not a failure.
    AlreadyThere,
    /// The stored file changed since it was deleted, or is not a regular file.
    Changed,
    /// No longer in Deleted files (restored or removed elsewhere).
    Missing,
    /// The original name cannot be rebuilt on this system.
    Unrepresentable,
    /// The original location is inside deleted-file storage or OneCopy's
    /// own data folder.
    Excluded,
    /// A folder on the original path is now a link.
    FolderIsLink,
    /// A file (or another non-folder) is where a folder of the original path
    /// was.
    FileInTheWay,
    /// A different drive is mounted on the original path.
    OtherDrive,
}

impl Skip {
    fn descriptor(self) -> &'static str {
        match self {
            // Never recorded: the file is where it belongs.
            Skip::AlreadyThere => "",
            Skip::Changed => CHANGED,
            Skip::Missing => MISSING,
            Skip::Unrepresentable | Skip::Excluded => "notice.restoreUnplaceable",
            Skip::FolderIsLink | Skip::FileInTheWay => FOLDER_BLOCKED,
            Skip::OtherDrive => OTHER_DRIVE,
        }
    }

    fn from_status(status: EntryStatus) -> Option<Self> {
        match status {
            EntryStatus::Restorable => None,
            EntryStatus::Changed => Some(Skip::Changed),
            EntryStatus::Unrepresentable => Some(Skip::Unrepresentable),
            EntryStatus::Excluded => Some(Skip::Excluded),
        }
    }
}

const MISSING: &str = "notice.restoreMissing";
const CHANGED: &str = "notice.restoreChanged";
const FOLDER_BLOCKED: &str = "notice.restoreFolderBlocked";
const OTHER_DRIVE: &str = "notice.restoreOtherDrive";
const OCCUPIED: &str = "notice.restoreOccupied";
const FAILED: &str = "notice.restoreFailed";

/// What planning found at the original path of one selected file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetState {
    Absent,
    /// A regular file with the same bytes as the stored one.
    Identical,
    /// Anything else: different content, a folder, a link.
    Occupied,
}

/// One selected file with what the edge observed about its way home.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub entry: TrashEntry,
    /// `<root>/<original relative path>`, when it can be placed.
    pub target: Option<PathBuf>,
    /// Folders on the way that do not exist, root-down.
    pub missing_folders: Vec<PathBuf>,
    /// A problem on the way that stops this file, if any.
    pub blocked: Option<Skip>,
    pub target_state: TargetState,
}

/// What happens to one selected file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Move the stored file to `target`; `renamed` when it is not the
    /// original name.
    Restore { target: PathBuf, renamed: bool },
    Skip(Skip),
}

#[derive(Clone, Debug)]
pub struct Step {
    pub entry: TrashEntry,
    pub action: Action,
}

/// The frozen plan: steps in execution order and the folders it recreates.
#[derive(Clone, Debug, Default)]
pub struct RestorePlan {
    pub steps: Vec<Step>,
    pub folders: Vec<PathBuf>,
}

/// What the review shows. It is needed only when something needs a decision
/// or a warning: a name conflict, a folder to recreate, a selected file that will be skipped, or a companion that will not pair
/// with its main file restored earlier under another name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReview {
    /// Every file with where it goes (relative to the root).
    pub files: Vec<ReviewFile>,
    /// Folders to recreate, relative to the root.
    pub folders: Vec<String>,
    /// Companions of a restored main file that stay in Deleted files.
    pub companions_left: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    pub id: String,
    pub original: Option<String>,
    /// Where the file goes, relative to the root; `None` when skipped.
    pub target: Option<String>,
    pub renamed: bool,
    pub skip: Option<Skip>,
    /// A companion coming back under a name that no longer pairs with its
    /// main file, which was restored earlier under another name: where that
    /// main file is, relative to the root.
    pub main_restored_as: Option<String>,
}

impl RestoreReview {
    pub fn needed(&self) -> bool {
        !self.folders.is_empty()
            || self
                .files
                .iter()
                .any(|file| {
                file.renamed
                    || file.skip.is_some()
                    || file.main_restored_as.is_some()
            })
    }
}

fn family_key(entry: &TrashEntry) -> String {
    entry.group.clone()
}

/// Decides every selected file's fate. Pure: `available(path)` answers
/// whether a suffixed candidate name is free on disk (a name the filesystem
/// cannot hold counts as free; its rename then fails as that one file).
///
/// - Files that cannot be restored are skipped with their reason.
/// - Files whose original path already holds the same bytes are skipped as
///   already there.
/// - The rest move in families: one deleted item's files in one folder. A
///   family whose original names are taken, on disk or by a newer family in
///   this selection, takes the smallest Rename suffix free for all of them.
/// - Families are placed newest deletion first, so the newest version of a
///   path gets the original name.
pub fn plan_restore(
    candidates: &[Candidate],
    names: FolderNames,
    style: RenameStyle,
    available: &mut dyn FnMut(&Path) -> bool,
) -> RestorePlan {
    let mut plan = RestorePlan::default();
    let mut families: Vec<(String, Vec<&Candidate>)> = Vec::new();
    let mut family_index: HashMap<String, usize> = HashMap::new();
    let mut skipped = Vec::new();
    for candidate in candidates {
        let skip = candidate
            .blocked
            .or(Skip::from_status(candidate.entry.status))
            .or(candidate.target.is_none().then_some(Skip::Unrepresentable))
            .or((candidate.target_state == TargetState::Identical).then_some(Skip::AlreadyThere));
        if let Some(skip) = skip {
            skipped.push(Step {
                entry: candidate.entry.clone(),
                action: Action::Skip(skip),
            });
            continue;
        }
        let target = candidate.target.as_ref().expect("placeable");
        let folder = target
            .parent()
            .map(|parent| names.key(parent))
            .unwrap_or_default();
        let key = format!("{}\0{}", family_key(&candidate.entry), folder.to_string_lossy());
        match family_index.get(&key) {
            Some(&index) => families[index].1.push(candidate),
            None => {
                family_index.insert(key.clone(), families.len());
                families.push((key, vec![candidate]));
            }
        }
    }
    // Newest deletion first; ties by id so the order is stable.
    let newest = |members: &Vec<&Candidate>| {
        members
            .iter()
            .map(|candidate| candidate.entry.deleted_at_utc.clone())
            .max()
            .unwrap_or_default()
    };
    families.sort_by(|(left_key, left), (right_key, right)| {
        newest(right).cmp(&newest(left)).then_with(|| left_key.cmp(right_key))
    });

    let mut reserved: HashSet<std::ffi::OsString> = HashSet::new();
    let mut folders: Vec<PathBuf> = Vec::new();
    let mut known_folders: HashSet<PathBuf> = HashSet::new();
    for (_, mut members) in families {
        // The main file leads its companions.
        members.sort_by_key(|candidate| {
            (
                candidate.entry.role == Some(TrashRole::Companion),
                candidate.entry.id.clone(),
            )
        });
        let originals: Vec<&PathBuf> = members
            .iter()
            .map(|candidate| candidate.target.as_ref().expect("placeable"))
            .collect();
        let mut claimed_here = HashSet::new();
        let conflict = members.iter().zip(&originals).any(|(candidate, target)| {
            let key = names.key(target);
            candidate.target_state == TargetState::Occupied
                || reserved.contains(&key)
                || !claimed_here.insert(key)
        });
        let targets: Vec<PathBuf> = if conflict {
            (2..=1_000_000u32)
                .find_map(|number| {
                    let candidates = originals
                        .iter()
                        .map(|target| file_names::renamed(target, number, style))
                        .collect::<Option<Vec<_>>>()?;
                    let mut keys = HashSet::new();
                    let free = candidates.iter().all(|candidate| {
                        let key = names.key(candidate);
                        !reserved.contains(&key) && keys.insert(key) && available(candidate)
                    });
                    free.then_some(candidates)
                })
                .unwrap_or_default()
        } else {
            originals.iter().map(|target| (*target).clone()).collect()
        };
        if targets.len() != members.len() {
            // No free suffix (or a name that is not Unicode): these files
            // cannot be placed and stay in Deleted files.
            for candidate in members {
                skipped.push(Step {
                    entry: candidate.entry.clone(),
                    action: Action::Skip(Skip::Unrepresentable),
                });
            }
            continue;
        }
        for (candidate, target) in members.into_iter().zip(targets) {
            reserved.insert(names.key(&target));
            for folder in &candidate.missing_folders {
                if known_folders.insert(folder.clone()) {
                    folders.push(folder.clone());
                }
            }
            plan.steps.push(Step {
                entry: candidate.entry.clone(),
                action: Action::Restore {
                    target,
                    renamed: conflict,
                },
            });
        }
    }
    folders.sort_by_key(|folder| folder.components().count());
    plan.folders = folders;
    plan.steps.extend(skipped);
    plan
}

/// The name stem companions pair by (the library's rule: same folder, same
/// stem ignoring case).
fn pairing_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The review the plan needs, relative to `root`.
pub fn review_of(plan: &RestorePlan, root: &Path, selected_all: &[TrashEntry]) -> RestoreReview {
    let relative = |path: &Path| {
        path.strip_prefix(root)
            .map(|below| {
                below
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_else(|_| path.to_string_lossy().into_owned())
    };
    let files = plan
        .steps
        .iter()
        .map(|step| {
            let (target, renamed, skip) = match &step.action {
                Action::Restore { target, renamed } => (Some(target), *renamed, None),
                Action::Skip(skip) => (None, false, Some(*skip)),
            };
            // Blueprint case 24: pairing is by folder and name stem, so a
            // companion restored under a stem its restored main no longer
            // has comes back alone.
            let main_restored_as = match (target, &step.entry.main_restored_as) {
                (Some(target), Some(main))
                    if step.entry.role == Some(TrashRole::Companion)
                        && pairing_stem(target) != pairing_stem(Path::new(main)) =>
                {
                    Some(main.clone())
                }
                _ => None,
            };
            ReviewFile {
                id: step.entry.id.clone(),
                original: step.entry.original_relative.clone(),
                target: target.map(|target| relative(target)),
                renamed,
                skip,
                main_restored_as,
            }
        })
        .collect();
    // A restored main whose companions from the same deletion were not
    // selected: they stay in Deleted files.
    let restored_families: HashSet<String> = plan
        .steps
        .iter()
        .filter(|step| {
            matches!(step.action, Action::Restore { .. })
                && step.entry.role != Some(TrashRole::Companion)
        })
        .map(|step| family_key(&step.entry))
        .collect();
    let chosen: HashSet<&str> = plan.steps.iter().map(|step| step.entry.id.as_str()).collect();
    let companions_left = selected_all
        .iter()
        .filter(|entry| {
            entry.role == Some(TrashRole::Companion)
                && !chosen.contains(entry.id.as_str())
                && restored_families.contains(&family_key(entry))
        })
        .map(|entry| {
            entry
                .original_relative
                .clone()
                .unwrap_or_else(|| entry.stored_name.clone())
        })
        .collect();
    RestoreReview {
        files,
        folders: plan.folders.iter().map(|folder| relative(folder)).collect(),
        companions_left,
    }
}

/// Names exactly what the plan will do: every file's action and target, the
/// stored file it observed, and the folders it recreates. Confirming a review
/// sends it back; a different token means something changed.
pub fn plan_token(root: &Path, plan: &RestorePlan) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut field = |value: &[u8]| {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value);
    };
    field(root.as_os_str().as_encoded_bytes());
    for step in &plan.steps {
        field(step.entry.id.as_bytes());
        field(&step.entry.size.to_le_bytes());
        field(step.entry.deleted_at_utc.as_bytes());
        match &step.action {
            Action::Restore { target, renamed } => {
                field(b"restore");
                field(target.as_os_str().as_encoded_bytes());
                field(&[u8::from(*renamed)]);
            }
            Action::Skip(skip) => {
                field(b"skip");
                field(format!("{skip:?}").as_bytes());
            }
        }
    }
    for folder in &plan.folders {
        field(folder.as_os_str().as_encoded_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

// ---------------------------------------------------------------------------
// Observation (the edge)

/// Observes one selected entry's way home: its target, the folders on the
/// way, and whether the target already holds the same bytes.
pub fn observe(
    root: &Path,
    entry: &TrashEntry,
    volume: &dyn Fn(&Path) -> io::Result<u64>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Candidate, String> {
    let mut candidate = Candidate {
        entry: entry.clone(),
        target: None,
        missing_folders: Vec::new(),
        blocked: None,
        target_state: TargetState::Absent,
    };
    if !entry.status.restorable() {
        return Ok(candidate);
    }
    let Some(target) = entry
        .original_relative
        .as_deref()
        .and_then(|relative| trash::target_in_root(root, relative).ok())
    else {
        return Ok(candidate);
    };
    candidate.target = Some(target.clone());
    let Some(parent) = target.parent() else {
        candidate.blocked = Some(Skip::Unrepresentable);
        return Ok(candidate);
    };
    // Every folder from just below the root down to the target's own.
    let mut chain: Vec<PathBuf> = parent
        .ancestors()
        .take_while(|ancestor| *ancestor != root)
        .map(Path::to_path_buf)
        .collect();
    chain.reverse();
    let mut deepest_existing = root.to_path_buf();
    for folder in chain {
        if !candidate.missing_folders.is_empty() {
            candidate.missing_folders.push(folder);
            continue;
        }
        match volume_io::symlink_metadata(&folder) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                candidate.blocked = Some(Skip::FolderIsLink);
                return Ok(candidate);
            }
            Ok(metadata) if metadata.is_dir() => deepest_existing = folder,
            Ok(_) => {
                candidate.blocked = Some(Skip::FileInTheWay);
                return Ok(candidate);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                candidate.missing_folders.push(folder)
            }
            Err(error) if file_names::is_name_error(&error) => {
                candidate.blocked = Some(Skip::Unrepresentable);
                return Ok(candidate);
            }
            Err(error) => return Err(format!("could not inspect {}: {error}", folder.display())),
        }
    }
    let day_dir = Path::new(&entry.stored_path)
        .parent()
        .ok_or("a stored file has no day folder")?;
    let home = volume(&deepest_existing)
        .map_err(|error| format!("could not inspect {}: {error}", deepest_existing.display()))?;
    let stored = volume(day_dir)
        .map_err(|error| format!("could not inspect {}: {error}", day_dir.display()))?;
    if home != stored {
        candidate.blocked = Some(Skip::OtherDrive);
        return Ok(candidate);
    }
    if !candidate.missing_folders.is_empty() {
        return Ok(candidate);
    }
    candidate.target_state = match volume_io::symlink_metadata(&target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound || file_names::is_name_error(&error) => {
            TargetState::Absent
        }
        Err(error) => return Err(format!("could not inspect {}: {error}", target.display())),
        Ok(metadata) if metadata.file_type().is_file() && metadata.len() == entry.size => {
            if same_bytes(Path::new(&entry.stored_path), &target, entry.size, cancelled)? {
                TargetState::Identical
            } else {
                TargetState::Occupied
            }
        }
        Ok(_) => TargetState::Occupied,
    };
    Ok(candidate)
}

fn same_bytes(stored: &Path, target: &Path, size: u64, cancelled: &dyn Fn() -> bool) -> Result<bool, String> {
    let hash = |path: &Path| -> Result<String, String> {
        let (mut file, _) = crate::file_identity::open_regular_nofollow(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        crate::hashing::full_hash_file_cancellable(&mut file, size, cancelled, &mut |_, _| {})
            .map_err(|error| {
                if error.kind() == io::ErrorKind::Interrupted && cancelled() {
                    crate::scanner::CANCELLED.to_string()
                } else {
                    format!("could not read {}: {error}", path.display())
                }
            })
    };
    Ok(hash(stored)? == hash(target)?)
}

/// Whether a suffixed name is free on disk. A name the filesystem cannot hold
/// counts as free; its rename then fails as that one file.
pub fn name_available(path: &Path) -> bool {
    volume_io::symlink_metadata(path).is_err_and(|error| {
        error.kind() == io::ErrorKind::NotFound || file_names::is_name_error(&error)
    })
}

// ---------------------------------------------------------------------------
// Execution

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub cancelled: bool,
    /// Why the unstarted remainder was stopped, if it was.
    pub error: Option<String>,
    /// Where each restored file now is.
    pub restored: Vec<String>,
    pub already_present: u64,
    /// Files not restored, including `unknown`.
    pub failed: u64,
    /// Renames given up on while the drive was not responding.
    pub unknown: u64,
    pub unstarted: u64,
    pub files_total: u64,
    pub plan_token: Option<String>,
    pub requires_review: bool,
    pub plan_changed: bool,
    pub review: Option<RestoreReview>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoreProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub failures: u64,
}

/// A failed step: the Issue descriptor that explains it and the detail.
enum StepFailure {
    /// This file only.
    File(&'static str, String),
    /// The drive or root cannot take more: this file was not moved, and the
    /// rest stops. The reason is the drive's, never this file's own.
    Stop(String),
    /// Given up on while the drive was not responding: outcome unknown, and
    /// nothing further goes to this drive.
    Unknown(String),
}

fn stops_remainder(error: &io::Error, root: &Path) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ReadOnlyFilesystem | io::ErrorKind::StorageFull
    ) || volume_io::wait_failure(error).is_some()
        || !volume_io::is_dir(root).unwrap_or(false)
}

/// Creates one missing folder on the way and proves it is a real folder
/// inside the root. Another process creating it first is fine.
fn ensure_folder(folder: &Path, root: &Path) -> Result<(), StepFailure> {
    match volume_io::create_dir(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // The folder above went away since planning.
            return Err(StepFailure::File(MISSING, error.to_string()));
        }
        Err(error) => {
            let message = format!("could not create {}: {error}", folder.display());
            return Err(if stops_remainder(&error, root) {
                StepFailure::Stop(message)
            } else {
                StepFailure::File(FAILED, message)
            });
        }
    }
    match volume_io::symlink_metadata(folder) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StepFailure::File(
            FOLDER_BLOCKED,
            format!("{} is a link", folder.display()),
        )),
        Ok(metadata) if metadata.is_dir() => {
            match crate::path_identity::directory_is_within(folder, root) {
                Ok(true) => Ok(()),
                Ok(false) => Err(StepFailure::File(
                    FOLDER_BLOCKED,
                    format!("{} leads outside its root", folder.display()),
                )),
                Err(error) => Err(StepFailure::File(FOLDER_BLOCKED, error)),
            }
        }
        Ok(_) => Err(StepFailure::File(
            FOLDER_BLOCKED,
            format!("{} is not a folder", folder.display()),
        )),
        Err(error) => Err(StepFailure::File(FOLDER_BLOCKED, error.to_string())),
    }
}

fn restore_one(
    root: &Path,
    entry: &TrashEntry,
    target: &Path,
    volume: &dyn Fn(&Path) -> io::Result<u64>,
) -> Result<(), StepFailure> {
    let stored = Path::new(&entry.stored_path);
    let day_dir = stored.parent().unwrap_or(root);
    // The stored file must still be the one the record describes.
    match volume_io::symlink_metadata(stored) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(StepFailure::File(MISSING, error.to_string()))
        }
        Err(error) if stops_remainder(&error, root) => {
            return Err(StepFailure::Stop(error.to_string()))
        }
        Err(error) => return Err(StepFailure::File(CHANGED, error.to_string())),
        Ok(metadata) => {
            let unchanged = metadata.file_type().is_file()
                && metadata.len() == entry.size
                && Some(trash::mtime_ms(&metadata)) == entry.mtime_ms;
            if !unchanged {
                return Err(StepFailure::File(
                    CHANGED,
                    "the stored file changed since it was deleted".to_string(),
                ));
            }
        }
    }
    let parent = target.parent().unwrap_or(root);
    let chain: Vec<&Path> = {
        let mut chain: Vec<&Path> = parent
            .ancestors()
            .take_while(|ancestor| *ancestor != root)
            .collect();
        chain.reverse();
        chain
    };
    for folder in chain {
        match volume_io::symlink_metadata(folder) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(StepFailure::File(
                    FOLDER_BLOCKED,
                    format!("{} is a link", folder.display()),
                ))
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(StepFailure::File(
                    FOLDER_BLOCKED,
                    format!("{} is not a folder", folder.display()),
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => ensure_folder(folder, root)?,
            Err(error) if stops_remainder(&error, root) => {
                return Err(StepFailure::Stop(error.to_string()))
            }
            Err(error) => return Err(StepFailure::File(FOLDER_BLOCKED, error.to_string())),
        }
    }
    let same_drive = match (volume(parent), volume(day_dir)) {
        (Ok(home), Ok(stored)) => home == stored,
        (Err(error), _) | (_, Err(error)) => {
            return Err(if stops_remainder(&error, root) {
                StepFailure::Stop(error.to_string())
            } else {
                StepFailure::File(OTHER_DRIVE, error.to_string())
            })
        }
    };
    if !same_drive {
        return Err(StepFailure::File(
            OTHER_DRIVE,
            format!("a different drive is at {}", parent.display()),
        ));
    }
    match crate::fs_publish::rename_no_replace(stored, target) {
        Ok(()) => {}
        Err(error) if volume_io::outcome_unknown(&error) => {
            return Err(StepFailure::Unknown(error.to_string()))
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(StepFailure::File(
                OCCUPIED,
                format!("{} was occupied after the review", target.display()),
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(StepFailure::File(MISSING, error.to_string()))
        }
        Err(error) if stops_remainder(&error, root) => {
            return Err(StepFailure::Stop(error.to_string()))
        }
        Err(error) => {
            return Err(StepFailure::File(
                FAILED,
                format!("could not restore to {}: {error}", target.display()),
            ))
        }
    }
    for folder in [parent, day_dir] {
        if let Err(error) = crate::fs_publish::sync_directory(folder) {
            logging::warn(
                "restore directory sync failed after the move completed",
                json!({ "path": folder, "error": { "message": error.to_string() } }),
            );
        }
    }
    let restored_to = target
        .strip_prefix(root)
        .map(|below| {
            below
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default();
    trash::append_restored(day_dir, &entry.stored_name, &restored_to);
    Ok(())
}

/// Executes a frozen plan inside `root`, file by file. Cancellation is
/// honoured between files; each rename is one bounded step and is never
/// interrupted. A failure tied to one file becomes an Issue and the batch
/// continues; a read-only or full drive, a root that went away, or a drive
/// that stopped answering stops the unstarted remainder. Restored files in a
/// source root are re-read into the index at the end, whatever ended the
/// batch.
#[allow(clippy::too_many_arguments)]
pub fn execute(
    conn: &rusqlite::Connection,
    root: &Path,
    plan: &RestorePlan,
    settings: &crate::scanner::ScanSettings,
    volume: &dyn Fn(&Path) -> io::Result<u64>,
    cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(RestoreProgress),
) -> Result<(RestoreOutcome, u64), String> {
    let files_total = plan.steps.len() as u64;
    let mut outcome = RestoreOutcome {
        files_total,
        ..RestoreOutcome::default()
    };
    let mut progress = RestoreProgress {
        files_done: 0,
        files_total,
        failures: 0,
    };
    on_progress(progress);
    let mut restored_dirs: Vec<PathBuf> = Vec::new();
    let mut known_dirs: HashSet<PathBuf> = HashSet::new();
    let mut result = Ok(());
    for (index, step) in plan.steps.iter().enumerate() {
        if cancelled() {
            outcome.cancelled = true;
            outcome.unstarted = files_total - index as u64;
            break;
        }
        let stored = step.entry.stored_path.clone();
        let failure = match &step.action {
            Action::Skip(Skip::AlreadyThere) => {
                outcome.already_present += 1;
                None
            }
            Action::Skip(skip) => Some(StepFailure::File(skip.descriptor(), String::new())),
            Action::Restore { target, .. } => match restore_one(root, &step.entry, target, volume) {
                Ok(()) => {
                    outcome.restored.push(target.to_string_lossy().into_owned());
                    if let Some(parent) = target.parent() {
                        if known_dirs.insert(parent.to_path_buf()) {
                            restored_dirs.push(parent.to_path_buf());
                        }
                    }
                    None
                }
                Err(failure) => Some(failure),
            },
        };
        progress.files_done += 1;
        let mut stop = None;
        if let Some(failure) = failure {
            outcome.failed += 1;
            progress.failures += 1;
            let recorded = match failure {
                StepFailure::File(key, message) => {
                    record(conn, &stored, RESTORE_ERROR, key, &message)
                }
                StepFailure::Stop(message) => {
                    let recorded = record(conn, &stored, RESTORE_ERROR, FAILED, &message);
                    stop = Some(message);
                    recorded
                }
                StepFailure::Unknown(message) => {
                    outcome.unknown += 1;
                    let recorded = record(
                        conn,
                        &stored,
                        RESTORE_OUTCOME_UNKNOWN,
                        "notice.restoreOutcomeUnknown",
                        &message,
                    );
                    stop = Some(message);
                    recorded
                }
            };
            // An Issue that cannot be saved stops the batch (the promised
            // failure record is a shared requirement).
            if let Err(error) = recorded {
                outcome.error = Some(error.clone());
                outcome.unstarted = files_total - progress.files_done;
                result = Err(error);
                on_progress(progress);
                break;
            }
        }
        on_progress(progress);
        if let Some(message) = stop {
            logging::warn(
                "restore stopped: the drive cannot take more",
                json!({ "root": root, "error": { "message": &message } }),
            );
            outcome.error = Some(message);
            outcome.unstarted = files_total - progress.files_done;
            break;
        }
    }
    let changed = reindex(conn, root, &restored_dirs, settings);
    logging::info(
        "restore",
        json!({
            "restored": outcome.restored.len(),
            "alreadyPresent": outcome.already_present,
            "failed": outcome.failed,
            "unknown": outcome.unknown,
            "unstarted": outcome.unstarted,
            "cancelled": outcome.cancelled,
        }),
    );
    result.map(|()| (outcome, changed))
}

fn record(conn: &rusqlite::Connection, path: &str, kind: &str, key: &str, message: &str) -> Result<(), String> {
    logging::warn(
        "restore failed for one file",
        json!({ "path": path, "kind": kind, "error": { "message": message } }),
    );
    crate::index_store::upsert_issue_with_descriptor(conn, Some(path), kind, Some(key), None, message)
        .map(|_| ())
}

/// Re-reads the folders files were restored into, when they lie in a source,
/// so the library updates without waiting for the watcher. Only the index
/// facts ordinary discovery records; identity, dates and pairing are left as
/// durable debt for file-information completion.
fn reindex(
    conn: &rusqlite::Connection,
    root: &Path,
    dirs: &[PathBuf],
    settings: &crate::scanner::ScanSettings,
) -> u64 {
    // Quitting: the file is in place, and the next launch's source check or
    // watcher indexes it; the bounded exit does not wait for a re-read.
    if crate::app_lifecycle::shutting_down() {
        return 0;
    }
    let root_text = root.to_string_lossy();
    let in_source = settings
        .source_dirs
        .iter()
        .any(|source| crate::scanner::directory_belongs_to_root(&root_text, source));
    if !in_source || dirs.is_empty() {
        return 0;
    }
    match crate::watcher::restat_batch(conn, dirs, settings, &|| Ok(())) {
        Ok(pass) => pass.changed(),
        Err(error) => {
            logging::warn(
                "restored folders could not be re-read",
                json!({ "error": { "message": error } }),
            );
            0
        }
    }
}

/// Observes every selected id against a fresh listing, in selection order.
/// An id the listing no longer has (its file was restored or removed
/// elsewhere) is a candidate that is skipped as missing.
pub fn candidates(
    root: &Path,
    listing: &trash::TrashListing,
    ids: &[String],
    volume: &dyn Fn(&Path) -> io::Result<u64>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Candidate>, String> {
    let location = root.join(trash::TRASH_DIR_NAME);
    let by_id: HashMap<&str, &TrashEntry> = listing
        .entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let mut seen = HashSet::new();
    let mut observed = Vec::new();
    for id in ids.iter().filter(|id| seen.insert(id.as_str())) {
        if cancelled() {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        observed.push(match by_id.get(id.as_str()) {
            Some(entry) => observe(root, entry, volume, cancelled)?,
            None => missing_candidate(missing_entry(id, &location)),
        });
    }
    Ok(observed)
}

/// A selected id the listing no longer has: its file was restored or removed
/// elsewhere.
fn missing_entry(id: &str, location: &Path) -> TrashEntry {
    let (day, name) = id.split_once('/').unwrap_or(("", id));
    TrashEntry {
        id: id.to_string(),
        day: day.to_string(),
        stored_name: name.to_string(),
        stored_path: location.join(day).join(name).to_string_lossy().into_owned(),
        original_relative: None,
        deleted_at_utc: String::new(),
        size: 0,
        mtime_ms: None,
        group: id.to_string(),
        kind: None,
        operation: None,
        item: None,
        role: None,
        moved_to: None,
        status: EntryStatus::Changed,
        main_restored_as: None,
    }
}

/// The candidate for an id the listing no longer has.
fn missing_candidate(entry: TrashEntry) -> Candidate {
    Candidate {
        entry,
        target: None,
        missing_folders: Vec::new(),
        blocked: Some(Skip::Missing),
        target_state: TargetState::Absent,
    }
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: the seam that stands in for a volume
// without an exclusive rename is private to the crate.
#[path = "../tests/unit/restore.rs"]
mod tests;

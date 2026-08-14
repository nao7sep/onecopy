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
//! is resolved by an exclusive-create suffix loop (`image1.2.jpg`, …); the
//! manifest line records both the original path and the actual stored name, so
//! restore mapping stays exact even in the suffixed case.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::logging;

pub const TRASH_DIR_NAME: &str = ".onecopy-trash";
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
    if !file.is_absolute() {
        return Err(format!("trash requires an absolute path: {}", file.display()));
    }

    let volume_root = volume_root_of(file)?;
    let trash_root = trash_root_for(&volume_root, app_root)?;
    // Day folders use the FILENAME timestamp form (`yyyymmdd-utc`), never a
    // slice of the serialized ISO form — the timestamp conventions' date-only
    // grammar, with `-utc` carried because the files inside are the user's
    // own originals and cannot carry it themselves.
    let day = format!("{}-utc", &logging::filename_stamp_now()[..8]);
    let day_dir = trash_root.join(&day);

    // Original path relative to its volume root, preserved under the day
    // folder so the structure alone is restorable by hand.
    let relative = file
        .strip_prefix(&volume_root)
        .map_err(|_| {
            format!(
                "{} is not under its own volume root {}",
                file.display(),
                volume_root.display()
            )
        })?
        .to_path_buf();
    let target = day_dir.join(&relative);
    let parent = target
        .parent()
        .ok_or_else(|| "trash target has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    let stored = rename_with_suffix_loop(file, &target)?;

    let record = TrashedRecord {
        original_path: file.to_string_lossy().to_string(),
        stored_path: stored.to_string_lossy().to_string(),
        content_hash: content_hash.map(|h| h.to_string()),
        deleted_at_utc: logging::now_iso_millis(),
    };
    append_manifest(&day_dir, &record)?;

    #[cfg(windows)]
    hide_windows(&trash_root);

    Ok(record)
}

/// Renames `src` over `target`, falling back to `stem.2.ext`, `stem.3.ext`, …
/// under exclusive create when the name is taken. Returns the stored path.
fn rename_with_suffix_loop(src: &Path, target: &Path) -> Result<PathBuf, String> {
    let mut candidate = target.to_path_buf();
    let mut counter = 2u32;
    loop {
        // Exclusive create claims the name; the rename then replaces the
        // zero-byte claim with the real file (same directory, atomic).
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => {
                std::fs::rename(src, &candidate).map_err(|e| {
                    let _ = std::fs::remove_file(&candidate);
                    format!("trash rename failed for {}: {e}", src.display())
                })?;
                return Ok(candidate);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if counter > 9999 {
                    return Err(format!(
                        "could not find a free trash name for {}",
                        src.display()
                    ));
                }
                candidate = suffixed_name(target, counter);
                counter += 1;
            }
            Err(err) => return Err(err.to_string()),
        }
    }
}

/// `image1.jpg` + 2 → `image1.2.jpg`; extensionless names get `name.2`.
fn suffixed_name(target: &Path, counter: u32) -> PathBuf {
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let name = match target.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}.{counter}.{ext}"),
        None => format!("{stem}.{counter}"),
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
        .open(day_dir.join("manifest.jsonl"))
        .map_err(|e| e.to_string())?;
    // not recorded: the manifest is trash-side audit data, append-mode by
    // construction, never managed text.
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(|e| e.to_string())
}

/// The trash root for a volume: `<volume root>/.onecopy-trash`, except the
/// home volume, which uses `<app root>/trash` (macOS forbids writing at `/`).
fn trash_root_for(volume_root: &Path, app_root: &Path) -> Result<PathBuf, String> {
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
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Prefix(prefix)), Some(std::path::Component::RootDir)) => {
            Ok(PathBuf::from(prefix.as_os_str()).join(std::path::MAIN_SEPARATOR.to_string()))
        }
        _ => Err(format!("no volume prefix in {}", path.display())),
    }
}

// Walks up to the nearest existing ancestor, so a just-deleted sibling or a
// not-yet-created leaf never breaks volume detection.
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

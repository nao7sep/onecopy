//! Best-effort whole-store snapshots completed at the process lifecycle edges.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

const ARCHIVES: &str = "backups";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format_version: i64,
    written_at_utc: String,
    entries: Vec<Entry>,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Entry {
    original_path: PathBuf,
    entry_name: String,
    hash: Option<String>,
    skipped: Option<String>,
}

struct Cleanup(PathBuf, bool);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let result = if self.1 { std::fs::remove_dir_all(&self.0) } else { std::fs::remove_file(&self.0) };
        if let Err(error) = result {
            crate::logging::warn("archive cleanup failed", serde_json::json!({ "path": self.0, "error": { "message": error.to_string() } }));
        }
    }
}

pub(crate) fn launch(root: &Path) {
    let root = root.to_path_buf();
    complete(move || prepare_launch(&root));
}

fn prepare_launch(root: &Path) -> Result<(), String> {
    let directory = root.join(ARCHIVES);
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let marker = directory.join(".running");
    let unclean = marker.exists();
    File::create(&marker).map_err(|e| e.to_string())?;
    if unclean {
        // The process instance lock is already held before startup: any
        // archive lock left by this root's previous process is crash debris.
        remove_optional(&directory.join(".lock"))?;
        for entry in std::fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("archive-") && name.ends_with(".tmp") && entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                std::fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
            }
        }
        run(root)?;
    }
    Ok(())
}

pub(crate) fn clean_exit() {
    let Ok(root) = crate::paths::data_root() else { return; };
    complete(move || {
        crate::activity::close();
        crate::records::close();
        crate::backup_store::close();
        finish_archive(&root)
    });
}

fn finish_archive(root: &Path) -> Result<(), String> {
    run(root)?;
    remove_optional(&root.join(ARCHIVES).join(".running"))
}

fn complete(work: impl FnOnce() -> Result<(), String> + Send + 'static) {
    let spawned = std::thread::Builder::new().name("onecopy-archive".into()).spawn(work);
    let result = match spawned {
        Ok(worker) => worker.join()
            .unwrap_or_else(|payload| Err(crate::failure_runtime::panic_message(payload))),
        Err(error) => Err(error.to_string()),
    };
    if let Err(error) = result {
        crate::logging::warn("binary archive skipped", serde_json::json!({ "error": { "message": error } }));
    }
}

fn remove_optional(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn archives(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = std::fs::read_dir(directory).map_err(|e| e.to_string())?
        .map(|entry| entry.map(|entry| entry.path())).collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    files.retain(|path| path.extension().is_some_and(|extension| extension == "zip"));
    files.sort();
    Ok(files)
}

fn snapshot(path: &Path, target: &Path) -> Result<String, String> {
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    let mut destination = Connection::open(target).map_err(|e| e.to_string())?;
    let backup = rusqlite::backup::Backup::new(&source, &mut destination).map_err(|e| e.to_string())?;
    loop {
        match backup.step(128).map_err(|e| e.to_string())? {
            rusqlite::backup::StepResult::Done => break,
            rusqlite::backup::StepResult::More => {},
            _ => return Err("SQLite store is locked".into()),
        }
    }
    drop(backup);
    drop(destination);
    drop(source);
    let mut file = File::open(target).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 { break; }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn run(root: &Path) -> Result<bool, String> {
    let directory = root.join(ARCHIVES);
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let lock = directory.join(".lock");
    let _file = match OpenOptions::new().write(true).create_new(true).open(&lock) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    let _lock_cleanup = Cleanup(lock, false);
    let staging = directory.join(format!("archive-{}.tmp", crate::nanoid::generate()?));
    std::fs::create_dir(&staging).map_err(|e| e.to_string())?;
    let _staging_cleanup = Cleanup(staging.clone(), true);
    let entries = crate::storage::ARCHIVED_STORES.into_iter().map(|(path, name)| {
        let original_path = root.join(path);
        let copied = snapshot(&original_path, &staging.join(name));
        let (hash, skipped) = match copied { Ok(hash) => (Some(hash), None), Err(error) => (None, Some(error)) };
        Entry { original_path, entry_name: name.into(), hash, skipped }
    }).collect::<Vec<_>>();
    let skipped = entries.iter().filter(|entry| entry.skipped.is_some()).collect::<Vec<_>>();
    if !skipped.is_empty() {
        crate::logging::warn("binary archive stores skipped", serde_json::json!({ "stores": skipped }));
    }
    if entries.iter().all(|entry| entry.hash.is_none()) { return Ok(false); }
    let previous = archives(&directory)?;
    if let Some(latest) = previous.last() {
        let file = File::open(latest).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let manifest: serde_json::Value = serde_json::from_reader(archive.by_name("manifest.json").map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        // An archive a newer OneCopy wrote, or one without its marker, is
        // never read as this one's predecessor: a new archive is written
        // beside it.
        if crate::formats::json_version(&manifest).is_ok_and(|version| version <= crate::formats::ARCHIVE_MANIFEST) {
            let previous: Vec<Entry> = serde_json::from_value(manifest["entries"].clone()).map_err(|e| e.to_string())?;
            if previous == entries { return Ok(false); }
        }
    }
    let manifest = Manifest { format_version: crate::formats::ARCHIVE_MANIFEST, written_at_utc: crate::logging::now_iso_millis(), entries };
    let mut zip = zip::ZipWriter::new(File::create(staging.join("archive.zip")).map_err(|e| e.to_string())?);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for entry in &manifest.entries {
        if entry.hash.is_some() {
            zip.start_file(&entry.entry_name, options).map_err(|e| e.to_string())?;
            std::io::copy(&mut File::open(staging.join(&entry.entry_name)).map_err(|e| e.to_string())?, &mut zip).map_err(|e| e.to_string())?;
        }
    }
    zip.start_file("manifest.json", options).map_err(|e| e.to_string())?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    zip.finish().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())?;
    let target = directory.join(format!("{}.zip", chrono::Utc::now().format("%Y%m%d-%H%M%S-utc")));
    std::fs::hard_link(staging.join("archive.zip"), &target).map_err(|e| e.to_string())?;
    let mut kept = previous;
    kept.push(target);
    for old in thinned_out(&kept, chrono::Utc::now()) {
        std::fs::remove_file(old).map_err(|e| e.to_string())?;
    }
    Ok(true)
}

fn archive_time(path: &Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let stem = path.file_stem()?.to_str()?;
    chrono::NaiveDateTime::parse_from_str(stem, "%Y%m%d-%H%M%S-utc").ok().map(|time| time.and_utc())
}

/// The archives the data-lifecycle conventions' thinning schedule drops; the
/// newest, and any archive whose name carries no time, always stay.
fn thinned_out(archives: &[PathBuf], now: chrono::DateTime<chrono::Utc>) -> Vec<PathBuf> {
    use chrono::Datelike;
    let timed = archives
        .iter()
        .filter_map(|path| archive_time(path).map(|time| (path, time)))
        .collect::<Vec<_>>();
    let Some(newest) = timed.iter().map(|(_, time)| *time).max() else {
        return Vec::new();
    };
    timed
        .iter()
        .filter(|(_, time)| {
            let age = now - *time;
            if *time == newest || age <= chrono::Duration::days(21) {
                return false;
            }
            let bucket = |time: &chrono::DateTime<chrono::Utc>| {
                if age <= chrono::Duration::days(90) {
                    (0, time.year(), time.ordinal())
                } else if age <= chrono::Duration::days(1_095) {
                    let week = time.iso_week();
                    (1, week.year(), week.week())
                } else {
                    (2, time.year(), time.month())
                }
            };
            timed
                .iter()
                .any(|(_, other)| other > time && bucket(other) == bucket(time))
        })
        .map(|(path, _)| (*path).clone())
        .collect()
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises private snapshot, dedup,
// and lifecycle seams without widening the shipped module surface.
#[path = "../tests/unit/binary_archive.rs"]
mod tests;

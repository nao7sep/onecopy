//! Bounded, best-effort whole-store snapshots at the process lifecycle edges.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

const BUDGET: Duration = Duration::from_secs(2);
const ARCHIVES: &str = "backups/archives";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
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
    bounded(move |deadline| prepare_launch(&root, deadline));
}

fn prepare_launch(root: &Path, deadline: Instant) -> Result<(), String> {
    check_deadline(deadline)?;
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
            check_deadline(deadline)?;
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("archive-") && name.ends_with(".tmp") && entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                std::fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
            }
        }
        run(root, deadline)?;
    }
    Ok(())
}

pub(crate) fn clean_exit() {
    let Ok(root) = crate::paths::data_root() else { return; };
    bounded(move |deadline| {
        crate::activity::close();
        crate::backup_store::close();
        finish_archive(&root, deadline)
    });
}

fn finish_archive(root: &Path, deadline: Instant) -> Result<(), String> {
    run(root, deadline)?;
    check_deadline(deadline)?;
    remove_optional(&root.join(ARCHIVES).join(".running"))
}

fn bounded(work: impl FnOnce(Instant) -> Result<(), String> + Send + 'static) {
    let deadline = Instant::now() + BUDGET;
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("onecopy-archive".into()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(deadline)))
            .unwrap_or_else(|payload| Err(crate::failure_runtime::panic_message(payload)));
        let _ = tx.send(result);
    });
    let result = match spawned {
        Ok(_) => rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap_or_else(|e| Err(e.to_string())),
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

fn check_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline { Err("archive deadline exceeded".into()) } else { Ok(()) }
}

fn archives(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = std::fs::read_dir(directory).map_err(|e| e.to_string())?
        .map(|entry| entry.map(|entry| entry.path())).collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    files.retain(|path| path.extension().is_some_and(|extension| extension == "zip"));
    files.sort();
    Ok(files)
}

fn snapshot(path: &Path, target: &Path, deadline: Instant) -> Result<String, String> {
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    let mut destination = Connection::open(target).map_err(|e| e.to_string())?;
    let backup = rusqlite::backup::Backup::new(&source, &mut destination).map_err(|e| e.to_string())?;
    loop {
        check_deadline(deadline)?;
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
        check_deadline(deadline)?;
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 { break; }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn run(root: &Path, deadline: Instant) -> Result<bool, String> {
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
        let copied = snapshot(&original_path, &staging.join(name), deadline);
        let (hash, skipped) = match copied { Ok(hash) => (Some(hash), None), Err(error) => (None, Some(error)) };
        Entry { original_path, entry_name: name.into(), hash, skipped }
    }).collect::<Vec<_>>();
    check_deadline(deadline)?;
    let skipped = entries.iter().filter(|entry| entry.skipped.is_some()).collect::<Vec<_>>();
    if !skipped.is_empty() {
        crate::logging::warn("binary archive stores skipped", serde_json::json!({ "stores": skipped }));
    }
    let previous = archives(&directory)?;
    if let Some(latest) = previous.last() {
        let file = File::open(latest).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let manifest: Manifest = serde_json::from_reader(archive.by_name("manifest.json").map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if manifest.entries == entries { return Ok(false); }
    }
    let manifest = Manifest { written_at_utc: crate::logging::now_iso_millis(), entries };
    let mut zip = zip::ZipWriter::new(File::create(staging.join("archive.zip")).map_err(|e| e.to_string())?);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for entry in &manifest.entries {
        if entry.hash.is_some() {
            check_deadline(deadline)?;
            zip.start_file(&entry.entry_name, options).map_err(|e| e.to_string())?;
            std::io::copy(&mut File::open(staging.join(&entry.entry_name)).map_err(|e| e.to_string())?, &mut zip).map_err(|e| e.to_string())?;
        }
    }
    zip.start_file("manifest.json", options).map_err(|e| e.to_string())?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    zip.finish().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())?;
    check_deadline(deadline)?;
    let target = directory.join(format!("{}.zip", chrono::Utc::now().format("%Y%m%d-%H%M%S-utc")));
    std::fs::hard_link(staging.join("archive.zip"), target).map_err(|e| e.to_string())?;
    for old in previous.into_iter().rev().skip(9) {
        check_deadline(deadline)?;
        std::fs::remove_file(old).map_err(|e| e.to_string())?;
    }
    Ok(true)
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises private snapshot, dedup,
// and deadline seams without widening the shipped module surface.
#[path = "../tests/unit/binary_archive.rs"]
mod tests;

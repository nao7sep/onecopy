use super::*;
use std::time::Duration;

fn stores(root: &Path) {
    for (name, _) in crate::storage::ARCHIVED_STORES {
        let database = Connection::open(root.join(name)).unwrap();
        database.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE evidence (value TEXT); INSERT INTO evidence VALUES ('committed');").unwrap();
    }
}

fn read_archive(root: &Path) -> (Manifest, Vec<(String, Vec<u8>)>) {
    let files = archives(&root.join(ARCHIVES)).unwrap();
    let mut zip = zip::ZipArchive::new(File::open(files.last().unwrap()).unwrap()).unwrap();
    let manifest: Manifest = serde_json::from_reader(zip.by_name("manifest.json").unwrap()).unwrap();
    let bytes = manifest.entries.iter().filter(|entry| entry.hash.is_some()).map(|entry| {
        let mut bytes = Vec::new();
        zip.by_name(&entry.entry_name).unwrap().read_to_end(&mut bytes).unwrap();
        (entry.entry_name.clone(), bytes)
    }).collect();
    (manifest, bytes)
}

#[test]
fn sqlite_snapshots_restore_and_unchanged_content_deduplicates() {
    let root = tempfile::tempdir().unwrap();
    stores(root.path());
    assert!(run(root.path()).unwrap());
    let (manifest, entries) = read_archive(root.path());
    assert_eq!(entries.len(), crate::storage::ARCHIVED_STORES.len());
    assert!(manifest.written_at_utc.ends_with('Z'));
    for (name, bytes) in entries {
        let entry = manifest.entries.iter().find(|entry| entry.entry_name == name).unwrap();
        assert_eq!(entry.original_path, root.path().join(&name));
        assert_eq!(entry.hash, Some(hex::encode(Sha256::digest(&bytes))));
        assert!(entry.skipped.is_none());
        let restored = root.path().join(format!("restored-{name}"));
        std::fs::write(&restored, bytes).unwrap();
        let database = Connection::open(restored).unwrap();
        assert_eq!(database.query_row("SELECT value FROM evidence", [], |row| row.get::<_, String>(0)).unwrap(), "committed");
    }
    assert!(!run(root.path()).unwrap());
    assert_eq!(archives(&root.path().join(ARCHIVES)).unwrap().len(), 1);
    assert!(!root.path().join(ARCHIVES).join(".lock").exists());
}

#[test]
fn snapshots_include_committed_wal_bytes_without_copying_live_database_files() {
    let root = tempfile::tempdir().unwrap();
    let live = Connection::open(root.path().join("index.sqlite3")).unwrap();
    live.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE evidence (value TEXT); INSERT INTO evidence VALUES ('committed');").unwrap();
    assert!(root.path().join("index.sqlite3-wal").is_file());
    run(root.path()).unwrap();
    let (manifest, entries) = read_archive(root.path());
    assert_eq!(entries.len(), 1);
    assert!(manifest.entries[0].skipped.is_none());
    let restored = root.path().join("restored.sqlite3");
    std::fs::write(&restored, &entries[0].1).unwrap();
    let database = Connection::open(restored).unwrap();
    assert_eq!(database.query_row("SELECT value FROM evidence", [], |row| row.get::<_, String>(0)).unwrap(), "committed");
}

#[test]
fn exclusive_archive_lock_skips_the_run() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join(ARCHIVES);
    std::fs::create_dir_all(&directory).unwrap();
    File::create(directory.join(".lock")).unwrap();
    assert!(!run(root.path()).unwrap());
    assert!(archives(&directory).unwrap().is_empty());
    assert!(directory.join(".lock").exists());
}

#[test]
fn new_launch_sets_the_marker_and_unclean_launch_archives_before_opening_stores() {
    let root = tempfile::tempdir().unwrap();
    prepare_launch(root.path()).unwrap();
    let directory = root.path().join(ARCHIVES);
    assert!(directory.join(".running").is_file());
    assert!(archives(&directory).unwrap().is_empty());
    stores(root.path());
    File::create(directory.join(".lock")).unwrap();
    let debris = directory.join("archive-crashed.tmp");
    std::fs::create_dir(&debris).unwrap();
    std::fs::write(debris.join("partial.zip"), b"partial").unwrap();
    prepare_launch(root.path()).unwrap();
    assert!(!debris.exists());
    assert_eq!(archives(&directory).unwrap().len(), 1);
    assert!(!directory.join(".lock").exists());
    assert!(directory.join(".running").exists());
}

#[test]
fn archives_are_thinned_by_age_and_the_newest_always_stays() {
    let now = chrono::NaiveDateTime::parse_from_str("20261003-120000-utc", "%Y%m%d-%H%M%S-utc").unwrap().and_utc();
    let names = [
        "20261001-080000-utc",  // within 21 days: every copy stays
        "20261001-090000-utc",
        "20260901-080000-utc",  // days 22-90: the last of each UTC day
        "20260901-200000-utc",
        "20260301-080000-utc",  // days 91-1,095: the last of each ISO week (Sunday ends week 9)
        "20260302-080000-utc",  // Monday starts week 10
        "20260304-080000-utc",
        "20230101-080000-utc",  // after 1,095 days: the last of each month
        "20230115-080000-utc",
        "not-a-time",
    ];
    let paths = names.iter().map(|name| PathBuf::from(format!("{name}.zip"))).collect::<Vec<_>>();
    let mut dropped = thinned_out(&paths, now)
        .into_iter()
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    dropped.sort();
    assert_eq!(dropped, ["20230101-080000-utc", "20260302-080000-utc", "20260901-080000-utc"]);
    // A lone old archive is the newest and stays.
    assert!(thinned_out(&paths[7..8], now).is_empty());
}

#[test]
fn a_run_thins_the_archives_beside_it() {
    let root = tempfile::tempdir().unwrap();
    stores(root.path());
    run(root.path()).unwrap();
    let directory = root.path().join(ARCHIVES);
    let first = archives(&directory).unwrap().pop().unwrap();
    for name in ["20000101-080000-utc", "20000101-090000-utc"] {
        std::fs::copy(&first, directory.join(format!("{name}.zip"))).unwrap();
    }
    std::fs::remove_file(first).unwrap();
    let changed = Connection::open(root.path().join("index.sqlite3")).unwrap();
    changed.execute("INSERT INTO evidence VALUES ('new')", []).unwrap();
    drop(changed);
    assert!(run(root.path()).unwrap());
    let files = archives(&directory).unwrap();
    assert_eq!(files.len(), 2, "one copy of that month and the new archive");
    assert!(directory.join("20000101-090000-utc.zip").exists());
    assert!(!directory.join(".lock").exists());
    assert!(std::fs::read_dir(directory).unwrap().all(|entry| !entry.unwrap().path().extension().is_some_and(|ext| ext == "tmp")));
}

#[test]
fn missing_or_unreadable_stores_publish_nothing_and_release_the_lock() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join(ARCHIVES);
    for unreadable in [false, true] {
        if unreadable {
            for (name, _) in crate::storage::ARCHIVED_STORES {
                std::fs::write(root.path().join(name), b"not a SQLite database").unwrap();
            }
        }
        assert!(!run(root.path()).unwrap());
        assert!(archives(&directory).unwrap().is_empty());
        assert!(!directory.join(".lock").exists());
        assert!(std::fs::read_dir(&directory).unwrap().next().is_none());
    }
}

#[test]
fn an_empty_run_leaves_previous_archives_unchanged() {
    let root = tempfile::tempdir().unwrap();
    stores(root.path());
    run(root.path()).unwrap();
    let directory = root.path().join(ARCHIVES);
    let before = archives(&directory).unwrap();
    let bytes = std::fs::read(&before[0]).unwrap();
    for (name, _) in crate::storage::ARCHIVED_STORES {
        std::fs::remove_file(root.path().join(name)).unwrap();
    }
    assert!(!run(root.path()).unwrap());
    assert_eq!(archives(&directory).unwrap(), before);
    assert_eq!(std::fs::read(&before[0]).unwrap(), bytes);
}

#[test]
fn archive_waits_for_the_worker_to_finish() {
    let (release, wait) = std::sync::mpsc::channel();
    let (started, start) = std::sync::mpsc::channel();
    let (done, completion) = std::sync::mpsc::channel();
    let caller = std::thread::spawn(move || {
        complete(move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            Ok(())
        });
        done.send(()).unwrap();
    });
    start.recv_timeout(Duration::from_secs(5)).unwrap();
    // Hold the worker past the removed two-second budget. Completion must
    // still wait for its release rather than leaving the archive behind.
    let premature = completion.recv_timeout(Duration::from_millis(2100));
    release.send(()).unwrap();
    caller.join().unwrap();
    assert!(matches!(premature, Err(std::sync::mpsc::RecvTimeoutError::Timeout)));
    assert!(completion.try_recv().is_ok());
}

#[test]
fn a_failed_clean_exit_preserves_the_running_marker() {
    let root = tempfile::tempdir().unwrap();
    stores(root.path());
    prepare_launch(root.path()).unwrap();
    let directory = root.path().join(ARCHIVES);
    let marker = directory.join(".running");
    let corrupt = directory.join("20000101-000000-utc.zip");
    std::fs::write(&corrupt, b"not a zip").unwrap();
    assert!(finish_archive(root.path()).is_err());
    assert!(marker.exists());
    std::fs::remove_file(corrupt).unwrap();
    finish_archive(root.path()).unwrap();
    assert!(!marker.exists());
    assert_eq!(archives(&directory).unwrap().len(), 1);
}

#[test]
fn an_empty_clean_exit_removes_the_running_marker_without_an_archive() {
    let root = tempfile::tempdir().unwrap();
    prepare_launch(root.path()).unwrap();
    let directory = root.path().join(ARCHIVES);
    finish_archive(root.path()).unwrap();
    assert!(!directory.join(".running").exists());
    assert!(archives(&directory).unwrap().is_empty());
}

#[test]
fn the_archive_manifest_carries_its_format_version() {
    let root = tempfile::tempdir().unwrap();
    stores(root.path());
    assert!(run(root.path()).unwrap());
    let (manifest, _) = read_archive(root.path());
    assert_eq!(manifest.format_version, crate::formats::ARCHIVE_MANIFEST);
    assert_eq!(crate::formats::ARCHIVE_MANIFEST, 1);
}

#[test]
fn an_archive_a_newer_onecopy_wrote_is_not_read_as_the_predecessor_and_stays() {
    let root = tempfile::tempdir().unwrap();
    stores(root.path());
    assert!(run(root.path()).unwrap());
    let directory = root.path().join(ARCHIVES);
    let written = archives(&directory).unwrap().pop().unwrap();
    let (manifest, entries) = read_archive(root.path());
    // The same content under a newer manifest, a few seconds older by name.
    let older_name = (chrono::Utc::now() - chrono::Duration::seconds(5)).format("%Y%m%d-%H%M%S-utc.zip").to_string();
    let newer = directory.join(older_name);
    {
        let mut zip = zip::ZipWriter::new(File::create(&newer).unwrap());
        for (name, bytes) in &entries {
            zip.start_file(name.as_str(), SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        let mut document = serde_json::to_value(&manifest).unwrap();
        document["formatVersion"] = serde_json::json!(2);
        zip.start_file("manifest.json", SimpleFileOptions::default()).unwrap();
        zip.write_all(&serde_json::to_vec(&document).unwrap()).unwrap();
        zip.finish().unwrap();
    }
    std::fs::remove_file(&written).unwrap();
    let newer_bytes = std::fs::read(&newer).unwrap();
    assert!(run(root.path()).unwrap(), "unchanged content is archived again beside a newer archive");
    let kept = archives(&directory).unwrap();
    assert_eq!(kept.len(), 2);
    assert_eq!(std::fs::read(&newer).unwrap(), newer_bytes);
}

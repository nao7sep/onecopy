use super::*;

fn deadline() -> Instant { Instant::now() + Duration::from_secs(10) }

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
fn both_sqlite_snapshots_restore_and_unchanged_content_deduplicates() {
    let root = tempfile::tempdir().unwrap();
    stores(root.path());
    assert!(run(root.path(), deadline()).unwrap());
    let (manifest, entries) = read_archive(root.path());
    assert_eq!(entries.len(), 2);
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
    assert!(!run(root.path(), deadline()).unwrap());
    assert_eq!(archives(&root.path().join(ARCHIVES)).unwrap().len(), 1);
    assert!(!root.path().join(ARCHIVES).join(".lock").exists());
}

#[test]
fn snapshots_include_committed_wal_bytes_without_copying_live_database_files() {
    let root = tempfile::tempdir().unwrap();
    let live = Connection::open(root.path().join("index.sqlite3")).unwrap();
    live.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE evidence (value TEXT); INSERT INTO evidence VALUES ('committed');").unwrap();
    assert!(root.path().join("index.sqlite3-wal").is_file());
    run(root.path(), deadline()).unwrap();
    let (manifest, entries) = read_archive(root.path());
    assert_eq!(entries.len(), 1);
    assert!(manifest.entries[1].skipped.is_some());
    let restored = root.path().join("restored.sqlite3");
    std::fs::write(&restored, &entries[0].1).unwrap();
    let database = Connection::open(restored).unwrap();
    assert_eq!(database.query_row("SELECT value FROM evidence", [], |row| row.get::<_, String>(0)).unwrap(), "committed");
}

#[test]
fn missing_stores_are_listed_and_exclusive_archive_lock_skips_the_run() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join(ARCHIVES);
    std::fs::create_dir_all(&directory).unwrap();
    File::create(directory.join(".lock")).unwrap();
    assert!(!run(root.path(), deadline()).unwrap());
    assert!(archives(&directory).unwrap().is_empty());
    std::fs::remove_file(directory.join(".lock")).unwrap();
    assert!(run(root.path(), deadline()).unwrap());
    let (manifest, entries) = read_archive(root.path());
    assert!(entries.is_empty());
    assert_eq!(manifest.entries.len(), 2);
    assert!(manifest.entries.iter().all(|entry| entry.skipped.is_some() && entry.hash.is_none()));
    assert!(!root.path().join("index.sqlite3").exists());
}

#[test]
fn new_launch_sets_the_marker_and_unclean_launch_archives_before_opening_stores() {
    let root = tempfile::tempdir().unwrap();
    prepare_launch(root.path(), deadline()).unwrap();
    let directory = root.path().join(ARCHIVES);
    assert!(directory.join(".running").is_file());
    assert!(archives(&directory).unwrap().is_empty());
    stores(root.path());
    File::create(directory.join(".lock")).unwrap();
    let debris = directory.join("archive-crashed.tmp");
    std::fs::create_dir(&debris).unwrap();
    std::fs::write(debris.join("partial.zip"), b"partial").unwrap();
    prepare_launch(root.path(), deadline()).unwrap();
    assert!(!debris.exists());
    assert_eq!(archives(&directory).unwrap().len(), 1);
    assert!(!directory.join(".lock").exists());
    assert!(directory.join(".running").exists());
}

#[test]
fn newest_ten_are_kept_and_expired_runs_publish_nothing() {
    let root = tempfile::tempdir().unwrap();
    stores(root.path());
    run(root.path(), deadline()).unwrap();
    let directory = root.path().join(ARCHIVES);
    let first = archives(&directory).unwrap().pop().unwrap();
    let old = directory.join("20000101-000000-utc.zip");
    std::fs::rename(first, &old).unwrap();
    for day in 2..=10 { std::fs::copy(&old, directory.join(format!("200001{day:02}-000000-utc.zip"))).unwrap(); }
    let changed = Connection::open(root.path().join("index.sqlite3")).unwrap();
    changed.execute("INSERT INTO evidence VALUES ('new')", []).unwrap();
    drop(changed);
    assert!(run(root.path(), deadline()).unwrap());
    let files = archives(&directory).unwrap();
    assert_eq!(files.len(), 10);
    assert!(!old.exists());
    assert!(run(root.path(), Instant::now()).is_err());
    assert_eq!(archives(&directory).unwrap(), files);
    assert!(!directory.join(".lock").exists());
    assert!(std::fs::read_dir(directory).unwrap().all(|entry| !entry.unwrap().path().extension().is_some_and(|ext| ext == "tmp")));
}

#[test]
fn archive_wait_is_bounded_when_an_external_operation_stalls() {
    let (release, wait) = std::sync::mpsc::channel();
    let started = Instant::now();
    bounded(move |_| { wait.recv().unwrap(); Ok(()) });
    assert!(started.elapsed() >= BUDGET);
    assert!(started.elapsed() < BUDGET + Duration::from_secs(1));
    release.send(()).unwrap();
}


#[test]
fn only_a_completed_clean_exit_removes_the_running_marker() {
    let root = tempfile::tempdir().unwrap();
    stores(root.path());
    prepare_launch(root.path(), deadline()).unwrap();
    let marker = root.path().join(ARCHIVES).join(".running");
    assert!(finish_archive(root.path(), Instant::now()).is_err());
    assert!(marker.exists());
    finish_archive(root.path(), deadline()).unwrap();
    assert!(!marker.exists());
    assert_eq!(archives(&root.path().join(ARCHIVES)).unwrap().len(), 1);
}

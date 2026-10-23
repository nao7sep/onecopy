use super::*;

#[test]
fn concurrent_setup_keeps_wal_and_leaves_readers_independent_of_writers() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("concurrent.sqlite3");
    let start = std::sync::Arc::new(std::sync::Barrier::new(8));
    let journal = std::sync::Arc::new(JournalSetup::new());
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            let start = start.clone();
            let journal = journal.clone();
            std::thread::spawn(move || {
                let conn = Connection::open(path).unwrap();
                start.wait();
                journal.configure(&conn, Duration::from_secs(5)).unwrap();
                assert_eq!(
                    conn.pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                        .unwrap(),
                    "wal"
                );
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }

    let writer = Connection::open(&path).unwrap();
    journal.configure(&writer, Duration::from_secs(5)).unwrap();
    writer.execute_batch("CREATE TABLE example(value INTEGER); INSERT INTO example VALUES(1); BEGIN IMMEDIATE; UPDATE example SET value = 2;").unwrap();
    let reader = Connection::open(&path).unwrap();
    journal
        .configure(&reader, Duration::from_millis(100))
        .unwrap();
    assert_eq!(
        reader
            .query_row("SELECT value FROM example", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    writer.execute_batch("COMMIT").unwrap();
    assert_eq!(
        reader
            .query_row("SELECT value FROM example", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn a_damaged_index_is_set_aside_but_an_unmarked_one_is_left_for_its_rebuild() {
    let root = tempfile::tempdir().unwrap();
    let index = root.path().join("index.sqlite3");
    Connection::open(&index).unwrap().execute_batch("CREATE TABLE paths (id INTEGER)").unwrap();
    assert!(set_aside_if_unreadable(&index, false).unwrap().is_none(), "unmarked: rebuilt in place");
    assert!(index.exists());

    std::fs::write(&index, b"not a database").unwrap();
    let record = set_aside_if_unreadable(&index, false).unwrap().expect("set aside");
    assert_eq!(record.file, "index.sqlite3");
    assert!(record.quarantined_to.ends_with("-utc.invalid"));
    assert!(!index.exists());
    assert_eq!(std::fs::read(&record.quarantined_to).unwrap(), b"not a database");
}

// A store another connection holds locked is busy, not damaged.
#[test]
fn a_locked_store_is_never_set_aside() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("index.sqlite3");
    let holder = Connection::open(&store).unwrap();
    holder.execute_batch("CREATE TABLE t (v); PRAGMA user_version = 1; BEGIN EXCLUSIVE; INSERT INTO t VALUES (1);").unwrap();
    let before = std::fs::read(&store).unwrap();

    assert!(set_aside_if_unreadable(&store, true).unwrap().is_none());
    assert!(store.exists());
    assert_eq!(std::fs::read(&store).unwrap(), before);
    holder.execute_batch("ROLLBACK").unwrap();
}

#[cfg(unix)]
#[test]
fn attach_preserves_native_filename_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(std::ffi::OsStr::from_bytes(b"store-\xff.sqlite3"));
    if !write_native_filename_fixture(&path, b"") { return; }
    let source = Connection::open(&path).unwrap();
    source.execute_batch("CREATE TABLE marker(value); INSERT INTO marker VALUES(42);").unwrap();
    let reader = Connection::open_in_memory().unwrap();
    reader.execute("ATTACH DATABASE CAST(?1 AS TEXT) AS native", [filename_bytes(&path).unwrap()]).unwrap();
    assert_eq!(reader.query_row("SELECT value FROM native.marker", [], |row| row.get::<_, i64>(0)).unwrap(), 42);
    assert!(!root.path().join("store-\u{fffd}.sqlite3").exists());
}

#[cfg(unix)]
#[test]
fn quarantine_preserves_native_store_and_sidecar_names() {
    use std::os::unix::ffi::OsStrExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(std::ffi::OsStr::from_bytes(b"store-\xff.sqlite3"));
    if !write_native_filename_fixture(&path, b"not a database") { return; }
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    std::fs::write(&wal, b"retained WAL").unwrap();
    let record = set_aside_if_unreadable(&path, false).unwrap().unwrap();
    let quarantined = crate::native_path::from_text(&record.quarantined_to);
    assert_eq!(std::fs::read(&quarantined).unwrap(), b"not a database");
    let mut quarantined_wal = quarantined.as_os_str().to_os_string();
    quarantined_wal.push("-wal");
    assert_eq!(std::fs::read(quarantined_wal).unwrap(), b"retained WAL");
    assert_eq!(record.file, crate::native_path::encode(path.file_name().unwrap()));
    assert!(!path.exists());
}

windows_test! {
    "Windows only: native UTF-16 paths",
    fn sqlite_filename_api_rejects_unpaired_utf16() {
    use std::os::windows::ffi::OsStringExt;
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&[67, 58, 92, 0xd800]));
    assert!(filename_bytes(&path).is_err());
}
}

#[cfg(unix)]
fn write_native_filename_fixture(path: &std::path::Path, bytes: &[u8]) -> bool {
    match std::fs::write(path, bytes) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput
            || (cfg!(target_os = "macos") && error.raw_os_error() == Some(92)) => {
            eprintln!("native filename fixture unsupported by this filesystem: {error}");
            false
        }
        Err(error) => panic!("could not create native filename fixture: {error}"),
    }
}

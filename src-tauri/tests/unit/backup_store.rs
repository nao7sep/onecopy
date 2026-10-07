use super::*;
use serial_test::serial;
fn unique_store_file(label: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new().prefix(&format!("onecopy-backupstore-{label}-")).tempdir().unwrap();
    let path = dir.path().join("backups.sqlite3");
    (dir, path)
}

// Opens a throwaway store, runs `body` against a direct connection to the same
// file for assertions, and always closes the singleton afterward.
// Serialization is provided by `#[serial(backup_store)]` on each caller, not by
// an in-module lock, so this group is also mutually exclusive with the lib.rs
// atomic-write test that shares the same key.
fn with_store<F: FnOnce(&Path)>(label: &str, body: F) {
    let (_directory, file) = unique_store_file(label);
    struct Close;
    impl Drop for Close { fn drop(&mut self) { close_for_test(); } }
    let _close = Close;
    init(file.clone());
    body(&file);
}

// A read-only view of every row for a path, in insert order, for assertions.
fn rows_for(file: &Path, path: &str) -> Vec<(Vec<u8>, String, i64, String)> {
    let conn = Connection::open(file).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT content, content_sha256, byte_size, written_at_utc \
             FROM backups WHERE path = ?1 ORDER BY id ASC",
        )
        .unwrap();
    let rows = stmt
        .query_map([path], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows
}

#[test]
#[serial(backup_store)]
fn content_blob_is_byte_identical_including_crlf_and_non_utf8() {
    with_store("blob-fidelity", |file| {
        // A CR/LF pair, a UTF-8 BOM, and a lone 0xFF byte (invalid UTF-8):
        // proves the BLOB stores raw bytes, never decoded/normalized text.
        let raw: &[u8] = &[0xEF, 0xBB, 0xBF, b'a', b'\r', b'\n', b'b', 0xFF];
        let p = "/abs/doc.json";
        record(Path::new(p), raw);

        let rows = rows_for(file, p);
        assert_eq!(rows.len(), 1);
        let (content, hash, byte_size, _written) = &rows[0];
        assert_eq!(content.as_slice(), raw, "content BLOB must be byte-identical");
        assert_eq!(*byte_size, raw.len() as i64);
        assert_eq!(hash, &sha256_hex(raw), "hash is over the raw bytes");
    });
}

#[test]
#[serial(backup_store)]
fn written_at_utc_is_serialized_iso_ms_not_the_filename_stamp() {
    with_store("iso-shape", |file| {
        let p = "/abs/a.json";
        record(Path::new(p), b"x");
        let rows = rows_for(file, p);
        let written = &rows[0].3;
        // Serialized ISO-8601-ms shape: yyyy-mm-ddThh:mm:ss.fffZ.
        assert!(
            written.len() == 24
                && &written[4..5] == "-"
                && &written[7..8] == "-"
                && &written[10..11] == "T"
                && &written[13..14] == ":"
                && &written[16..17] == ":"
                && &written[19..20] == "."
                && written.ends_with('Z'),
            "written_at_utc {written:?} must be serialized ISO-8601-ms (2026-07-06T04:05:12.345Z)"
        );
        // Must NOT be the yyyymmdd-hhmmss-fff-utc filename stamp.
        assert!(!written.ends_with("-utc"), "must not be the filename stamp");
        assert!(!written.contains("-utc"), "must not be the filename stamp");
    });
}

#[test]
#[serial(backup_store)]
fn dedup_skips_an_unchanged_re_save() {
    with_store("dedup", |file| {
        let p = "/abs/b.json";
        record(Path::new(p), b"same");
        record(Path::new(p), b"same"); // identical -> deduped, no new row
        assert_eq!(rows_for(file, p).len(), 1, "an unchanged re-save writes no row");
    });
}

#[test]
#[serial(backup_store)]
fn a_changed_save_and_a_revert_each_insert_a_row() {
    with_store("changed-and-revert", |file| {
        let p = "/abs/c.json";
        record(Path::new(p), b"v1");
        record(Path::new(p), b"v2"); // changed -> new row
        record(Path::new(p), b"v1"); // revert to v1: differs from the LATEST (v2) -> new row
        let rows = rows_for(file, p);
        assert_eq!(rows.len(), 3, "changed save and revert each insert a row");
        assert_eq!(rows[0].0, b"v1");
        assert_eq!(rows[1].0, b"v2");
        assert_eq!(rows[2].0, b"v1"); // the revert is recorded as the new version it is
    });
}

#[test]
#[serial(backup_store)]
fn dedup_is_per_path_not_global() {
    with_store("per-path", |file| {
        // Identical content under two different paths each records (dedup is
        // per-path against that path's latest row, never global).
        record(Path::new("/abs/x.json"), b"same");
        record(Path::new("/abs/y.json"), b"same");
        assert_eq!(rows_for(file, "/abs/x.json").len(), 1);
        assert_eq!(rows_for(file, "/abs/y.json").len(), 1);
    });
}

#[test]
#[serial(backup_store)]
fn record_is_a_silent_no_op_when_the_store_never_opened() {
    // Best-effort: with the store disabled (never init'd / closed), a record
    // call must not panic and must simply do nothing.
    close_for_test(); // ensure disabled state
    record(Path::new("/abs/whatever.json"), b"data"); // must not panic
}

#[test]
#[serial(backup_store)]
fn record_never_panics_on_a_broken_connection() {
    // Best-effort under a store failure: prove the no-throw contract by
    // pointing init at an un-creatable path — the open fails, recording is
    // disabled, and a subsequent record is a silent no-op (never a panic,
    // never a crash). A path whose parent is a FILE, so create_dir_all +
    // open must fail.
    let dir = std::env::temp_dir().join(format!(
        "onecopy-backupstore-badpath-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file_as_parent = dir.join("not-a-dir");
    std::fs::write(&file_as_parent, b"x").unwrap(); // a regular file
    let store_file = file_as_parent.join("backups.sqlite3"); // parent is a file -> mkdir fails

    init(store_file); // open fails -> disabled, one warn logged, no panic
    record(Path::new("/abs/whatever.json"), b"data"); // silent no-op, no panic
    close_for_test();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_connections_serialize_the_latest_decision_before_insert() {
    let (_directory, file) = unique_store_file("cross-connection-dedup");
    let setup = open(&file).unwrap();
    drop(setup);
    let path = "/abs/concurrent.json";

    let file_a = file.clone();
    let first = std::thread::spawn(move || {
        let mut conn = open(&file_a).unwrap();
        try_record_with_after_latest(&mut conn, path, b"same", || {
            // Keep the write reservation across the decision edge so the
            // second connection has to read AFTER this commit. Removing or
            // delaying BEGIN IMMEDIATE makes it read the same predecessor.
            std::thread::sleep(std::time::Duration::from_millis(60));
        })
        .unwrap();
    });
    std::thread::sleep(std::time::Duration::from_millis(10));
    let file_b = file.clone();
    let second = std::thread::spawn(move || {
        let mut conn = open(&file_b).unwrap();
        try_record(&mut conn, path, b"same").unwrap();
    });
    let first_result = first.join();
    let second_result = second.join();
    assert!(first_result.is_ok());
    assert!(second_result.is_ok());

    assert_eq!(rows_for(&file, path).len(), 1);
}

#[test]
#[serial(backup_store)]
fn the_history_records_format_version_1() {
    with_store("format-version", |file| {
        let conn = Connection::open(file).unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, crate::formats::BACKUPS);
        assert_eq!(crate::formats::BACKUPS, 1);
    });
}

#[test]
#[serial(backup_store)]
fn a_history_without_its_marker_disables_recording_and_is_left_as_it_is() {
    let (_directory, file) = unique_store_file("unmarked");
    {
        let conn = Connection::open(&file).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO backups (path, content, content_sha256, byte_size, written_at_utc) \
             VALUES ('/abs/kept.json', x'00', 'h', 1, 't')",
            [],
        )
        .unwrap();
    }
    let before = std::fs::read(&file).unwrap();
    init(file.clone());
    record(Path::new("/abs/new.json"), b"new");
    close_for_test();
    assert_eq!(std::fs::read(&file).unwrap(), before, "nothing is written to it");
    assert_eq!(rows_for(&file, "/abs/kept.json").len(), 1);
    assert!(rows_for(&file, "/abs/new.json").is_empty());
}

#[test]
#[serial(backup_store)]
fn a_history_written_by_a_newer_onecopy_disables_recording_and_is_left_as_it_is() {
    let (_directory, file) = unique_store_file("newer");
    {
        let conn = Connection::open(&file).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
    }
    let before = std::fs::read(&file).unwrap();
    init(file.clone());
    record(Path::new("/abs/new.json"), b"new");
    close_for_test();
    assert_eq!(std::fs::read(&file).unwrap(), before, "nothing is written to it");
    assert!(rows_for(&file, "/abs/new.json").is_empty());
}

#[test]
fn existing_empty_and_negative_histories_are_left_uninitialized() {
    for version in [0, -1] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("backups.sqlite3");
        let conn = Connection::open(&file).unwrap();
        conn.pragma_update(None, "user_version", version).unwrap();
        drop(conn);
        let before = std::fs::read(&file).unwrap();
        assert!(open(&file).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }
}

#[test]
fn a_newer_marker_committed_before_history_setup_is_not_downgraded() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("backups.sqlite3");
    let result = open_before_setup(&file, || {
        let conn = Connection::open(&file).unwrap();
        conn.execute_batch("CREATE TABLE future(value); PRAGMA user_version = 2;").unwrap();
    });
    assert!(result.unwrap_err().contains("newer OneCopy"));
    let conn = Connection::open(&file).unwrap();
    assert_eq!(conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'backups'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

use super::*;
use serial_test::serial;
fn unique_store_file(label: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new().prefix(&format!("onecopy-backupstore-{label}-")).tempdir().unwrap();
    let path = dir.path().join("backups.sqlite3");
    (dir, path)
}

/// Runs one session against `file`: starts the writer, runs `body`, and stops
/// the writer after everything queued is written.
fn session(file: &Path, id: &str, body: impl FnOnce()) {
    struct Close;
    impl Drop for Close { fn drop(&mut self) { close_for_test(); } }
    let _close = Close;
    init(file.to_path_buf(), id.to_string());
    body();
}

// Every row for a path, in insert order: content, hash, size, time, session.
fn rows_for(file: &Path, path: &str) -> Vec<(Vec<u8>, String, i64, String, Option<String>)> {
    let conn = Connection::open(file).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT content, content_sha256, byte_size, written_at_utc, session_id \
             FROM backups WHERE path = ?1 ORDER BY id ASC",
        )
        .unwrap();
    let rows = stmt
        .query_map([path], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows
}

#[test]
#[serial(backup_store)]
fn content_blob_is_byte_identical_including_crlf_and_non_utf8() {
    let (_dir, file) = unique_store_file("blob-fidelity");
    // A CR/LF pair, a UTF-8 BOM, and a lone 0xFF byte (invalid UTF-8):
    // proves the BLOB stores raw bytes, never decoded/normalized text.
    let raw: &[u8] = &[0xEF, 0xBB, 0xBF, b'a', b'\r', b'\n', b'b', 0xFF];
    let p = "/abs/doc.json";
    session(&file, "s1", || record(Path::new(p), raw));

    let rows = rows_for(&file, p);
    assert_eq!(rows.len(), 1);
    let (content, hash, byte_size, _written, session) = &rows[0];
    assert_eq!(content.as_slice(), raw, "content BLOB must be byte-identical");
    assert_eq!(*byte_size, raw.len() as i64);
    assert_eq!(hash, &sha256_hex(raw), "hash is over the raw bytes");
    assert_eq!(session.as_deref(), Some("s1"));
}

#[test]
#[serial(backup_store)]
fn written_at_utc_is_serialized_iso_ms_not_the_filename_stamp() {
    let (_dir, file) = unique_store_file("iso-shape");
    let p = "/abs/a.json";
    session(&file, "s1", || record(Path::new(p), b"x"));
    let written = &rows_for(&file, p)[0].3;
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
    assert!(!written.contains("-utc"), "must not be the filename stamp");
}

#[test]
#[serial(backup_store)]
fn a_session_keeps_one_row_per_path_holding_its_last_save() {
    let (_dir, file) = unique_store_file("per-session");
    let p = "/abs/c.json";
    session(&file, "s1", || {
        record(Path::new(p), b"v1");
        assert!(drain(Duration::from_secs(5)));
        record(Path::new(p), b"v2");
        assert!(drain(Duration::from_secs(5)));
        record(Path::new(p), b"v3");
    });
    let rows = rows_for(&file, p);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, b"v3");
}

#[test]
#[serial(backup_store)]
fn each_session_adds_its_own_row_and_earlier_rows_never_change() {
    let (_dir, file) = unique_store_file("across-sessions");
    let p = "/abs/d.json";
    session(&file, "s1", || record(Path::new(p), b"v1"));
    session(&file, "s2", || record(Path::new(p), b"v2"));
    let rows = rows_for(&file, p);
    assert_eq!(rows.iter().map(|row| (row.0.clone(), row.4.clone())).collect::<Vec<_>>(), vec![
        (b"v1".to_vec(), Some("s1".to_string())),
        (b"v2".to_vec(), Some("s2".to_string())),
    ]);
}

#[test]
#[serial(backup_store)]
fn a_first_save_equal_to_an_earlier_session_writes_nothing_until_it_changes() {
    let (_dir, file) = unique_store_file("unchanged");
    let p = "/abs/b.json";
    session(&file, "s1", || record(Path::new(p), b"same"));
    session(&file, "s2", || record(Path::new(p), b"same"));
    assert_eq!(rows_for(&file, p).len(), 1, "an unchanged first save writes no row");
    session(&file, "s3", || {
        record(Path::new(p), b"same");
        assert!(drain(Duration::from_secs(5)));
        record(Path::new(p), b"changed");
    });
    let rows = rows_for(&file, p);
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[1].0.as_slice(), rows[1].4.as_deref()), (b"changed".as_slice(), Some("s3")));
}

#[test]
#[serial(backup_store)]
fn rows_are_per_path_not_global() {
    let (_dir, file) = unique_store_file("per-path");
    session(&file, "s1", || {
        record(Path::new("/abs/x.json"), b"same");
        record(Path::new("/abs/y.json"), b"same");
    });
    assert_eq!(rows_for(&file, "/abs/x.json").len(), 1);
    assert_eq!(rows_for(&file, "/abs/y.json").len(), 1);
}

#[test]
#[serial(backup_store)]
fn a_save_never_waits_for_a_locked_history() {
    let (_dir, file) = unique_store_file("locked");
    session(&file, "s1", || {
        assert!(drain(Duration::from_secs(5)));
        let blocker = Connection::open(&file).unwrap();
        blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let started = std::time::Instant::now();
        for _ in 0..20 {
            record(Path::new("/abs/locked.json"), b"bytes");
        }
        assert!(started.elapsed() < Duration::from_millis(50), "{:?}", started.elapsed());
        blocker.execute_batch("ROLLBACK").unwrap();
    });
}

#[test]
#[serial(backup_store)]
fn record_is_a_silent_no_op_when_the_store_never_opened() {
    close_for_test();
    record(Path::new("/abs/whatever.json"), b"data");
    assert!(drain(Duration::from_millis(10)));
}

#[test]
#[serial(backup_store)]
fn record_never_panics_when_the_store_cannot_open() {
    let dir = tempfile::tempdir().unwrap();
    let file_as_parent = dir.path().join("not-a-dir");
    std::fs::write(&file_as_parent, b"x").unwrap();
    let store_file = file_as_parent.join("backups.sqlite3"); // parent is a file
    session(&store_file, "s1", || record(Path::new("/abs/whatever.json"), b"data"));
}

#[test]
#[serial(backup_store)]
fn a_history_from_before_sessions_gains_them_and_keeps_its_rows() {
    let (_dir, file) = unique_store_file("before-sessions");
    {
        let conn = Connection::open(&file).unwrap();
        conn.execute_batch(
            "CREATE TABLE backups (id INTEGER PRIMARY KEY, path TEXT NOT NULL, content BLOB NOT NULL, \
             content_sha256 TEXT NOT NULL, byte_size INTEGER NOT NULL, written_at_utc TEXT NOT NULL); \
             CREATE INDEX idx_backups_path_id ON backups (path, id); PRAGMA user_version = 1;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO backups (path, content, content_sha256, byte_size, written_at_utc) \
             VALUES ('/abs/e.json', x'01', ?1, 1, 't')",
            [sha256_hex(&[1])],
        )
        .unwrap();
    }
    session(&file, "s1", || record(Path::new("/abs/e.json"), b"newer"));
    let rows = rows_for(&file, "/abs/e.json");
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].0.as_slice(), rows[0].4.as_deref()), (&[1u8][..], None));
    assert_eq!(rows[1].4.as_deref(), Some("s1"));
}

#[test]
#[serial(backup_store)]
fn the_history_records_format_version_1() {
    let (_dir, file) = unique_store_file("format-version");
    session(&file, "s1", || record(Path::new("/abs/f.json"), b"x"));
    let conn = Connection::open(&file).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, crate::formats::BACKUPS);
    assert_eq!(crate::formats::BACKUPS, 1);
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
    session(&file, "s1", || record(Path::new("/abs/new.json"), b"new"));
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
    session(&file, "s1", || record(Path::new("/abs/new.json"), b"new"));
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

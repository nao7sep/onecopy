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
fn records_finish_before_the_history_consumer_accepts_any_job() {
    close_for_test();
    struct Close;
    impl Drop for Close { fn drop(&mut self) { close_for_test(); } }
    let _close = Close;
    let (sender, receiver) = mpsc::channel();
    lock().sender = Some(sender);
    let completed = std::thread::scope(|scope| {
        let (done, finished) = mpsc::channel();
        let producer = scope.spawn(move || {
            for value in 0..20u8 {
                record(Path::new("/abs/locked.json"), &[value]);
            }
            done.send(()).unwrap();
        });
        // A deadlock watchdog, not a throughput target. The consumer cannot
        // accept any job before the producer reports that every save returned.
        let completed = finished.recv_timeout(Duration::from_secs(10));
        if completed.is_ok() {
            for value in 0..20u8 {
                match receiver.try_recv().unwrap() {
                    Job::Record(path, bytes) => {
                        assert_eq!(path, "/abs/locked.json");
                        assert_eq!(bytes, [value]);
                    }
                    Job::Drained(_) => panic!("unexpected drain"),
                }
            }
            assert!(matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)));
        }
        // Release a regressed blocking sender before joining it on failure.
        drop(receiver);
        producer.join().unwrap();
        completed
    });
    assert!(completed.is_ok(), "saves waited for the history consumer");
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
fn a_history_without_session_columns_is_refused_and_kept_unchanged() {
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
    let before = std::fs::read(&file).unwrap();
    assert!(open(&file).is_err());
    session(&file, "s1", || record(Path::new("/abs/e.json"), b"newer"));
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert!(!PathBuf::from(format!("{}-wal", file.display())).exists());
    let conn = Connection::open(&file).unwrap();
    let rows: Vec<Vec<u8>> = conn
        .prepare("SELECT content FROM backups ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows, vec![vec![1u8]]);
    let has_sessions = conn
        .prepare("SELECT 1 FROM pragma_table_info('backups') WHERE name = 'session_id'")
        .unwrap()
        .exists([])
        .unwrap();
    assert!(!has_sessions);
}

#[test]
#[serial(backup_store)]
fn the_history_records_its_format_version() {
    let (_dir, file) = unique_store_file("format-version");
    session(&file, "s1", || record(Path::new("/abs/f.json"), b"x"));
    let conn = Connection::open(&file).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, crate::formats::BACKUPS);
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
    drop(open(&file).unwrap());
    let result = open_before_setup(&file, |setup_path| {
        let conn = Connection::open(setup_path).unwrap();
        conn.execute_batch("DROP TABLE backups; CREATE TABLE future(value); PRAGMA user_version = 2;").unwrap();
    });
    let expected = crate::formats::NewerStore::check(&file, 2, crate::formats::BACKUPS).unwrap();
    assert_eq!(result.unwrap_err(), expected.to_string());
    let conn = Connection::open(&file).unwrap();
    assert_eq!(conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'backups'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn failed_first_setup_keeps_the_canonical_path_absent_and_restart_succeeds() {
    let (directory, file) = unique_store_file("failed-setup");
    let result = open_before_setup(&file, |setup_path| {
        assert!(!file.exists(), "a first setup must remain private");
        assert_ne!(setup_path, file);
        let conn = Connection::open(setup_path).unwrap();
        let journal: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0)).unwrap();
        assert_eq!(journal, "delete", "publication cannot depend on WAL sidecars");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(setup_path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Refuse an incompatible schema inside the setup transaction.
        conn.execute_batch("CREATE TABLE backups(value); PRAGMA user_version = 1;").unwrap();
    });
    assert!(result.is_err());
    assert!(!file.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    let conn = open(&file).unwrap();
    assert_eq!(conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), crate::formats::BACKUPS);
    assert_eq!(conn.query_row("SELECT count(*) FROM backups", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn interrupted_first_setup_never_publishes_an_empty_canonical_file() {
    let (directory, file) = unique_store_file("interrupted-setup");
    let interrupted = std::panic::catch_unwind(|| {
        let _ = open_before_setup(&file, |_| {
            assert!(!file.exists());
            panic!("simulated interruption before schema setup");
        });
    });
    assert!(interrupted.is_err());
    assert!(!file.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    drop(open(&file).unwrap());
}

#[test]
fn a_competing_canonical_creation_is_never_overwritten() {
    for contents in [b"".as_slice(), b"unknown competing contents".as_slice()] {
        let (directory, file) = unique_store_file("competing-creation");
        let result = open_before_setup(&file, |_| {
            std::fs::write(&file, contents).unwrap();
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&file).unwrap(), contents);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(open(&file).is_err(), "existing empty files have no ownership evidence");
        assert_eq!(std::fs::read(&file).unwrap(), contents);
    }
}


#[test]
fn an_obsolete_schema_committed_before_setup_is_not_repaired() {
    let (_directory, file) = unique_store_file("obsolete-before-setup");
    drop(open(&file).unwrap());
    let result = open_before_setup(&file, |setup_path| {
        let conn = Connection::open(setup_path).unwrap();
        conn.execute_batch(
            "DROP TABLE backups; CREATE TABLE backups(value); INSERT INTO backups VALUES ('kept');",
        ).unwrap();
    });
    assert!(result.is_err());
    let conn = Connection::open(&file).unwrap();
    assert_eq!(conn.query_row("SELECT value FROM backups", [], |row| row.get::<_, String>(0)).unwrap(), "kept");
    assert_eq!(conn.query_row("SELECT count(*) FROM pragma_table_info('backups')", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

// A records store OneCopy cannot read is set aside at launch so a fresh one
// starts; a deleted store is created again rather than attached empty.

use onecopy_lib::records;
use rusqlite::Connection;

fn store() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(records::RECORDS_DB_FILE_NAME);
    (dir, path)
}

fn invalid_siblings(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".invalid"))
        .collect();
    names.sort();
    names
}

#[test]
fn a_malformed_store_and_its_log_are_set_aside_with_a_seconds_stamp() {
    let (dir, path) = store();
    std::fs::write(&path, b"not a database at all, just bytes").unwrap();
    std::fs::write(dir.path().join("records.sqlite3-wal"), b"old log").unwrap();

    let record = records::set_aside_if_unreadable(&path).unwrap().expect("set aside");

    assert_eq!(record.file, "records.sqlite3");
    let set_aside = std::path::PathBuf::from(&record.quarantined_to);
    let name = set_aside.file_name().unwrap().to_string_lossy().into_owned();
    // records-yyyymmdd-hhmmss-utc.invalid
    assert_eq!(name.len(), "records-20261009-120000-utc.invalid".len(), "{name}");
    assert!(name.starts_with("records-") && name.ends_with("-utc.invalid"), "{name}");
    assert_eq!(std::fs::read(&set_aside).unwrap(), b"not a database at all, just bytes");
    assert_eq!(std::fs::read(format!("{}-wal", set_aside.display())).unwrap(), b"old log");
    assert!(!path.exists());
    assert!(!dir.path().join("records.sqlite3-wal").exists());

    let fresh = records::open(&path).unwrap();
    let lines: i64 = fresh.query_row("SELECT COUNT(*) FROM log_lines", [], |row| row.get(0)).unwrap();
    assert_eq!(lines, 0);
}

#[test]
fn a_schema_without_its_marker_is_set_aside() {
    let (dir, path) = store();
    Connection::open(&path).unwrap().execute_batch("CREATE TABLE log_lines (id INTEGER)").unwrap();

    assert!(records::set_aside_if_unreadable(&path).unwrap().is_some());
    assert_eq!(invalid_siblings(dir.path()).len(), 1);
}

#[test]
fn readable_newer_and_absent_stores_are_left_as_they_are() {
    let (dir, path) = store();
    assert!(records::set_aside_if_unreadable(&path).unwrap().is_none(), "absent");

    drop(records::open(&path).unwrap());
    assert!(records::set_aside_if_unreadable(&path).unwrap().is_none(), "current");

    // A healthy store whose newest writes are still only in its log.
    let writer = records::open(&path).unwrap();
    writer.execute_batch("PRAGMA wal_autocheckpoint = 0").unwrap();
    writer
        .execute(
            "INSERT INTO log_lines (session_id, time_utc, level, message, line) VALUES ('s', 't', 'info', 'm', '{}')",
            [],
        )
        .unwrap();
    assert!(dir.path().join("records.sqlite3-wal").exists());
    assert!(records::set_aside_if_unreadable(&path).unwrap().is_none(), "current with a log");
    drop(writer);

    Connection::open(&path).unwrap().pragma_update(None, "user_version", 999).unwrap();
    assert!(records::set_aside_if_unreadable(&path).unwrap().is_none(), "newer");
    assert!(path.exists());
    assert!(invalid_siblings(dir.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn a_set_aside_that_cannot_be_renamed_fails_naming_the_file() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, path) = store();
    std::fs::write(&path, b"garbage").unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = records::set_aside_if_unreadable(&path);

    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = result.expect_err("the rename cannot happen");
    assert!(error.contains("records.sqlite3"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), b"garbage");
}

#[test]
fn a_store_deleted_while_running_is_created_again_on_attach() {
    let (_dir, path) = store();
    let first = Connection::open_in_memory().unwrap();
    records::attach(&first, &path).unwrap();
    drop(first);
    std::fs::remove_file(&path).unwrap();
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }

    let second = Connection::open_in_memory().unwrap();
    records::attach(&second, &path).unwrap();

    let lines: i64 = second
        .query_row("SELECT COUNT(*) FROM records.log_lines", [], |row| row.get(0))
        .unwrap();
    assert_eq!(lines, 0);
}

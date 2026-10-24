use super::*;

#[test]
fn a_negative_index_is_rebuilt_but_a_newer_competing_setup_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite3");
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("CREATE TABLE obsolete(value); PRAGMA user_version = -1;").unwrap();
    drop(conn);
    let conn = open(&file).unwrap();
    assert_eq!(raw_user_version(&conn).unwrap(), crate::formats::INDEX);
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'obsolete'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    drop(conn);
    let competing = dir.path().join("competing.sqlite3");
    let result = open_before_setup(&competing, || {
        Connection::open(&competing).unwrap().execute_batch("CREATE TABLE future(value); PRAGMA user_version = 2;").unwrap();
    });
    let expected = crate::formats::NewerStore::check(&competing, 2, crate::formats::INDEX).unwrap();
    assert_eq!(result.unwrap_err(), expected.to_string());
    let conn = Connection::open(&competing).unwrap();
    assert_eq!(raw_user_version(&conn).unwrap(), 2);
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'contents'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn only_the_rebuildable_index_skips_waiting_for_the_disk() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(&dir.path().join("index.sqlite3")).unwrap();
    let synchronous = |schema: &str| -> i64 {
        conn.query_row(&format!("PRAGMA {schema}.synchronous"), [], |row| row.get(0)).unwrap()
    };
    assert_eq!(synchronous("main"), 1, "the index commits without waiting (NORMAL)");
    assert_eq!(synchronous("records"), 2, "the attached records keep FULL");
}


#[test]
fn obsolete_marked_index_shapes_rebuild_without_touching_sibling_data() {
    let obsolete_shapes = [
        "CREATE TABLE paths(id INTEGER); CREATE TABLE scan_dirs(root TEXT, configured_root TEXT NOT NULL);",
        "CREATE TABLE paths(id INTEGER, file_key TEXT); CREATE TABLE scan_dirs(root TEXT, configured_root TEXT);",
        "CREATE TABLE paths(id INTEGER, file_key TEXT); CREATE TABLE scan_dirs(root TEXT);",
        "CREATE TABLE paths(id INTEGER, file_key TEXT); CREATE UNIQUE INDEX idx_paths_native_identity ON paths(id); CREATE TABLE scan_dirs(root TEXT, configured_root TEXT NOT NULL, dirty INTEGER NOT NULL DEFAULT 0);",
    ];
    for obsolete in obsolete_shapes {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("index.sqlite3");
        let conn = Connection::open(&file).unwrap();
        conn.execute_batch(obsolete).unwrap();
        conn.execute_batch("CREATE TABLE obsolete(value); INSERT INTO obsolete VALUES ('discarded'); PRAGMA user_version = 1;").unwrap();
        drop(conn);
        let records = dir.path().join(crate::records::RECORDS_DB_FILE_NAME);
        let record_conn = crate::records::open(&records).unwrap();
        record_conn.execute_batch("CREATE TABLE retained(value); INSERT INTO retained VALUES ('record');").unwrap();
        drop(record_conn);
        let backup = dir.path().join(crate::backup_store::BACKUPS_DB_FILE_NAME);
        let external = dir.path().join("authored.txt");
        std::fs::write(&backup, b"backup sentinel").unwrap();
        std::fs::write(&external, b"authored sentinel").unwrap();
        let conn = open(&file).unwrap();
        assert!(current_shape(&conn).unwrap());
        assert_eq!(raw_user_version(&conn).unwrap(), crate::formats::INDEX);
        assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'obsolete'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(conn.query_row("SELECT value FROM records.retained", [], |row| row.get::<_, String>(0)).unwrap(), "record");
        assert_eq!(std::fs::read(&backup).unwrap(), b"backup sentinel");
        assert_eq!(std::fs::read(&external).unwrap(), b"authored sentinel");
    }
}

#[test]
fn a_competing_current_index_setup_is_retained_under_the_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite3");
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("CREATE TABLE obsolete(value); PRAGMA user_version = 1;").unwrap();
    drop(conn);
    let conn = open_before_setup(&file, || {
        let competing = open(&file).unwrap();
        competing.execute_batch("INSERT INTO contents(hash, byte_size, kind) VALUES ('winner', 1, 'image');").unwrap();
    }).unwrap();
    assert!(current_shape(&conn).unwrap());
    assert_eq!(conn.query_row("SELECT hash FROM contents", [], |row| row.get::<_, String>(0)).unwrap(), "winner");
}

#[test]
fn a_newer_competing_index_replaces_an_obsolete_shape_without_being_reset() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite3");
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("CREATE TABLE obsolete(value); PRAGMA user_version = 1;").unwrap();
    drop(conn);
    let result = open_before_setup(&file, || {
        Connection::open(&file).unwrap().execute_batch(
            "DROP TABLE obsolete; CREATE TABLE future(value); INSERT INTO future VALUES ('kept'); PRAGMA user_version = 2;",
        ).unwrap();
    });
    assert!(result.is_err());
    let conn = Connection::open(&file).unwrap();
    assert_eq!(raw_user_version(&conn).unwrap(), 2);
    assert_eq!(conn.query_row("SELECT value FROM future", [], |row| row.get::<_, String>(0)).unwrap(), "kept");
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'contents'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn unreadable_index_bytes_are_an_error_without_a_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite3");
    let bytes = b"not a SQLite database";
    std::fs::write(&file, bytes).unwrap();
    assert!(open(&file).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
}

#[test]
fn pre_native_identity_index_rebuilds_under_the_current_marker() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite3");
    let conn = open(&file).unwrap();
    conn.execute_batch("INSERT INTO paths (abs_path, dir_path, file_name, kind) VALUES ('/old', '/', 'old', 'other');
                        CREATE TABLE records.retained_native_test(value TEXT);
                        INSERT INTO records.retained_native_test VALUES ('retain');
                        DROP INDEX idx_paths_native_identity;").unwrap();
    assert!(!current_shape(&conn).unwrap());
    drop(conn);
    let conn = open(&file).unwrap();
    assert!(current_shape(&conn).unwrap());
    assert_eq!(raw_user_version(&conn).unwrap(), crate::formats::INDEX);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM paths", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(conn.query_row("SELECT value FROM records.retained_native_test", [], |row| row.get::<_, String>(0)).unwrap(), "retain");
    conn.execute("INSERT INTO paths (abs_path, dir_path, file_name, kind) VALUES ('/new', '/', 'new', 'other')", []).unwrap();
    assert!(conn.execute("INSERT INTO paths (abs_path, dir_path, file_name, kind) VALUES ('/new', '/', 'duplicate', 'other')", []).is_err());
    drop(conn);
    let conn = open(&file).unwrap();
    assert_eq!(conn.query_row("SELECT abs_path FROM paths", [], |row| row.get::<_, String>(0)).unwrap(), "/new");
}

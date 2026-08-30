use onecopy_lib::{index_store, indexed_file};

#[test]
fn logical_resolution_uses_oldest_valid_date_then_case_insensitive_path() {
    let dir = tempfile::tempdir().unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('same', 5, 'other');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms)
         VALUES
           ('/B/same.txt', '/B', 'same.txt', 'other', 'same', 200),
           ('/z/same.txt', '/z', 'same.txt', 'other', 'same', 100),
           ('/a/same.txt', '/a', 'same.txt', 'other', 'same', 100);",
    )
    .unwrap();

    assert_eq!(
        indexed_file::live_path(&conn, Some("same"), None).unwrap(),
        std::path::PathBuf::from("/a/same.txt")
    );
}

#[test]
fn provisional_resolution_accepts_only_one_live_indexed_identity() {
    let dir = tempfile::tempdir().unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute(
        "INSERT INTO paths (id, abs_path, dir_path, file_name, kind)
         VALUES (7, '/notes.txt', '/', 'notes.txt', 'other')",
        [],
    )
    .unwrap();

    assert_eq!(
        indexed_file::live_path(&conn, None, Some(7)).unwrap(),
        std::path::PathBuf::from("/notes.txt")
    );
    assert!(indexed_file::live_path(&conn, Some("same"), Some(7)).is_err());
    assert!(indexed_file::live_path(&conn, None, None).is_err());

    conn.execute("UPDATE paths SET missing = 1 WHERE id = 7", [])
        .unwrap();
    assert!(indexed_file::live_path(&conn, None, Some(7)).is_err());
}

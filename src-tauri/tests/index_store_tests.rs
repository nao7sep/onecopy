use onecopy_lib::index_store;

#[test]
fn rebuild_clears_reconstructible_library_facts_and_issues() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('hash', 4, 'image');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, missing)
         VALUES ('/photos/a.jpg', '/photos', 'a.jpg', 'image', 'hash', 0);
         INSERT INTO issues (path, kind, message, first_seen_utc, last_seen_utc)
         VALUES ('/photos/a.jpg', 'read-error', 'failed', 'now', 'now');
         INSERT INTO scan_dirs (root, last_completed_at_utc)
         VALUES ('/photos', 'now');",
    )
    .unwrap();

    index_store::clear_reconstructible(&conn).unwrap();

    for table in ["contents", "paths", "issues", "scan_dirs"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

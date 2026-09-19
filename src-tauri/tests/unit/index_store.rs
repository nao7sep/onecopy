use super::*;

#[test]
fn open_creates_schema_and_is_idempotent() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-index-")
        .tempdir()
        .unwrap();
    let db = dir.path().join("index.sqlite3");

    let conn = open(&db).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_REVISION
    );
    // Every table in the current schema exists.
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap();
    let tables: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    // Set EQUALITY, not a subset, so any table change is deliberate.
    let mut expected = vec![
        "analysis_receipts",
        "contents",
        "evidence",
        "issues",
        "logical_contents",
        "logical_projection_batch",
        "paths",
        "recent_notifications",
        "scan_dirs",
        "similar_group_members",
        "similar_groups",
        "similarity_dirty_buckets",
        "similarity_state",
        "visibility_directories",
        "visibility_ignored_names",
        "visibility_policy",
        "volumes",
    ];
    expected.sort_unstable();
    let actual: Vec<&str> = tables
        .iter()
        .map(String::as_str)
        .filter(|t| !t.starts_with("sqlite_"))
        .collect();
    assert_eq!(actual, expected, "the schema's table set changed");
    drop(stmt);
    drop(conn);

    // Re-opening an existing file is fine (IF NOT EXISTS schema).
    let conn = open(&db).unwrap();
    upsert_issue(&conn, Some("/x"), "test", "x").unwrap();
}

#[test]
fn reopening_a_current_index_does_not_publish_schema_writes() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-index-read-open-")
        .tempdir()
        .unwrap();
    let db = dir.path().join("index.sqlite3");
    let observer = open(&db).unwrap();
    let before: i64 = observer
        .pragma_query_value(None, "data_version", |row| row.get(0))
        .unwrap();

    drop(open(&db).unwrap());

    let after: i64 = observer
        .pragma_query_value(None, "data_version", |row| row.get(0))
        .unwrap();
    assert_eq!(
        after, before,
        "an ordinary connection open must not invalidate read caches"
    );
}

#[test]
fn an_earlier_disposable_schema_reconstructs_old_rows() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-index-upgrade-")
        .tempdir()
        .unwrap();
    let db = dir.path().join("index.sqlite3");
    let conn = open(&db).unwrap();
    conn.execute("INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 10, 'image')", [])
        .unwrap();
    conn.pragma_update(None, "user_version", 8)
        .unwrap();
    drop(conn);

    let conn = open(&db).unwrap();
    let old_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM contents WHERE hash = 'h1'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(old_rows, 0);
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_REVISION
    );
}

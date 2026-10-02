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
        "contents",
        "evidence",
        "face_checks",
        "faces",
        "issues",
        "logical_contents",
        "logical_projection_batch",
        "paths",
        "rebuild_keeps_results",
        "recent_notifications",
        "resolution_policy",
        "scan_dirs",
        "similar_group_members",
        "similar_groups",
        "similarity_dirty_buckets",
        "similarity_state",
        "transcripts",
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
fn foreign_keys_are_enforced_on_an_ordinary_open_not_only_on_an_upgrade() {
    // R1-11: a connection that opens an already-current index (no upgrade
    // branch runs) must still enforce foreign keys, since every command,
    // worker and watcher pass opens this way.
    let dir = tempfile::Builder::new()
        .prefix("onecopy-index-fk-")
        .tempdir()
        .unwrap();
    let db = dir.path().join("index.sqlite3");
    // First open creates the schema at the current revision.
    drop(open(&db).unwrap());
    // Second open takes the "already current" path with no upgrade branch.
    let conn = open(&db).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
            .unwrap(),
        1,
        "foreign keys must be enforced on every ordinary open"
    );
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 10, 'image')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO evidence (content_hash, path_id, source) VALUES ('h1', 999, 'metadata')",
        [],
    )
    .expect_err("an evidence row referencing a path that does not exist must be refused");
}

#[test]
fn an_index_from_an_earlier_revision_is_rebuilt_from_the_files() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-index-upgrade-")
        .tempdir()
        .unwrap();
    let db = dir.path().join("index.sqlite3");
    let conn = open(&db).unwrap();
    conn.execute("INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 10, 'image')", [])
        .unwrap();
    conn.pragma_update(None, "user_version", SCHEMA_REVISION - 1)
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


use onecopy_lib::index_store;

#[test]
fn a_new_index_records_format_version_1() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        onecopy_lib::formats::INDEX
    );
    assert_eq!(onecopy_lib::formats::INDEX, 1);
}

#[test]
fn an_index_written_by_a_newer_onecopy_is_reported_by_name_and_left_as_it_is() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch("INSERT INTO contents (hash, byte_size, kind) VALUES ('retained', 1, 'image'); PRAGMA user_version = 2;").unwrap();
    drop(conn);
    let before = std::fs::read(&db).unwrap();
    let error = index_store::open(&db).expect_err("a newer index is not opened");
    assert!(error.contains(&db.to_string_lossy().into_owned()) && error.contains("newer"), "{error}");
    assert_eq!(
        onecopy_lib::formats::sqlite_newer(&db, onecopy_lib::formats::INDEX)
            .unwrap()
            .map(|newer| (newer.file, newer.version)),
        Some(("index.sqlite3".to_string(), 2))
    );
    assert_eq!(std::fs::read(&db).unwrap(), before, "the newer index is not written");
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM contents", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn rebuild_clears_reconstructible_library_facts_and_closes_issues() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('hash', 4, 'image');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, missing)
         VALUES ('/photos/a.jpg', '/photos', 'a.jpg', 'image', 'hash', 0);
         INSERT INTO scan_dirs (root, last_completed_at_utc)
         VALUES ('/photos', 'now');
         INSERT INTO transcripts (content_hash, model, model_version, text, segments, created_at_utc)
         VALUES ('hash', 'm', 'v', 'kept', '[]', 'now');
         INSERT INTO face_checks (content_hash, model, model_version, face_count, checked_at_utc)
         VALUES ('hash', 'm', 'v', 1, 'now');
         INSERT INTO faces (content_hash, x1, y1, x2, y2, confidence, model, model_version)
         VALUES ('hash', 0, 0, 1, 1, 0.9, 'm', 'v');",
    )
    .unwrap();
    index_store::upsert_issue(&conn, Some("/photos/a.jpg"), "read-error", "failed").unwrap();

    index_store::clear_reconstructible(&conn, false, false).unwrap();
    let rebuilt: (i64, i64) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM active_issues),
                    (SELECT COUNT(*) FROM records.issue_events WHERE event = 'rebuilt')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(rebuilt, (0, 1), "a rebuild closes the open Issues and keeps their records");
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap()
    };
    for table in ["transcripts", "face_checks", "faces"] {
        assert_eq!(count(table), 1, "a rebuild keeps {table} by default");
    }
    index_store::clear_reconstructible(&conn, true, false).unwrap();
    assert_eq!((count("transcripts"), count("face_checks"), count("faces")), (0, 1, 1));
    index_store::clear_reconstructible(&conn, false, true).unwrap();
    assert_eq!((count("face_checks"), count("faces")), (0, 0));

    for table in [
        "contents",
        "paths",
        "logical_contents",
        "similar_groups",
        "similar_group_members",
        "similarity_dirty_buckets",
        "similarity_state",
        "scan_dirs",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

#[test]
fn results_go_when_their_content_leaves_the_library() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('gone', 4, 'audio'), ('kept', 4, 'audio');
         INSERT INTO transcripts (content_hash, model, model_version, text, segments, created_at_utc)
         VALUES ('gone', 'm', 'v', 'a', '[]', 'now'), ('kept', 'm', 'v', 'b', '[]', 'now');
         INSERT INTO face_checks (content_hash, model, model_version, face_count, checked_at_utc)
         VALUES ('gone', 'm', 'v', 1, 'now'), ('kept', 'm', 'v', 0, 'now');
         INSERT INTO faces (content_hash, x1, y1, x2, y2, confidence, model, model_version)
         VALUES ('gone', 0, 0, 1, 1, 0.9, 'm', 'v');
         DELETE FROM contents WHERE hash = 'gone';",
    )
    .unwrap();
    let left: Vec<String> = conn
        .prepare("SELECT content_hash FROM transcripts")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(left, ["kept"]);
    let faces: (i64, i64) = conn
        .query_row("SELECT (SELECT COUNT(*) FROM face_checks WHERE content_hash = 'gone'), (SELECT COUNT(*) FROM faces)", [], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap();
    assert_eq!(faces, (0, 0));
}

/// (R4.1 finding 3) The section a logical item lands in must follow its
/// representative copy's own kind, never `contents.kind` — which records
/// only whichever copy was hashed first and has no product meaning (a
/// backup `.bak` copy hashed before its `.jpg` twin must not push the item
/// into Other files, and the reverse must not pull an Other item into
/// Images). Both directions are asserted against a single logical item each,
/// with `contents.kind` deliberately set opposite to the representative.
#[test]
fn logical_contents_kind_follows_the_representative_copy_not_contents_kind() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 4, 'other'), ('h2', 4, 'image');
         INSERT INTO paths (id, abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES
             (1, '/r/photo.jpg.bak', '/r', 'photo.jpg.bak', 'other', 'h1', 2000, 'metadata'),
             (2, '/r/photo.jpg', '/r', 'photo.jpg', 'image', 'h1', 1000, 'metadata'),
             (3, '/r/clip.mov', '/r', 'clip.mov', 'image', 'h2', 2000, 'metadata'),
             (4, '/r/clip.mov.bak', '/r', 'clip.mov.bak', 'other', 'h2', 1000, 'metadata');",
    )
    .unwrap();
    let kind = |hash: &str| {
        conn.query_row(
            "SELECT kind FROM logical_contents WHERE content_hash = ?1",
            [hash],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
    };
    // h1: contents.kind is 'other', but the earlier-dated (representative)
    // copy is the '.jpg' image — the section must be Images.
    assert_eq!(kind("h1"), "image");
    // h2: contents.kind is 'image', but the earlier-dated (representative)
    // copy is the '.bak' — the section must be Other files.
    assert_eq!(kind("h2"), "other");
}

#[test]
fn open_creates_schema_and_is_idempotent() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-index-")
        .tempdir()
        .unwrap();
    let db = dir.path().join("index.sqlite3");

    let conn = index_store::open(&db).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        onecopy_lib::formats::INDEX
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
        "library_choices",
        "logical_contents",
        "logical_projection_batch",
        "paths",
        "rebuild_keeps_results",
        "scan_dirs",
        "similar_group_members",
        "similar_groups",
        "similarity_dirty_buckets",
        "similarity_state",
        "transcripts",
        "visibility_directories",
        "visibility_ignored_names",
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
    let conn = index_store::open(&db).unwrap();
    index_store::upsert_issue(&conn, Some("/x"), "test", "x").unwrap();
}

#[test]
fn reopening_a_current_index_does_not_publish_schema_writes() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-index-read-open-")
        .tempdir()
        .unwrap();
    let db = dir.path().join("index.sqlite3");
    let observer = index_store::open(&db).unwrap();
    let before: i64 = observer
        .pragma_query_value(None, "data_version", |row| row.get(0))
        .unwrap();

    drop(index_store::open(&db).unwrap());

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
    // First open creates the schema at the current format version.
    drop(index_store::open(&db).unwrap());
    // Second open takes the "already current" path with no upgrade branch.
    let conn = index_store::open(&db).unwrap();
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
fn an_index_without_its_marker_is_rebuilt_from_the_files() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch("INSERT INTO contents (hash, byte_size, kind) VALUES ('stale', 1, 'image'); PRAGMA user_version = 0;").unwrap();
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM contents", [], |row| row.get::<_, i64>(0)).unwrap(),
        0
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(),
        onecopy_lib::formats::INDEX
    );
}

fn projection_plan(conn: &rusqlite::Connection) -> Vec<String> {
    let mut statement = conn
        .prepare("EXPLAIN QUERY PLAN SELECT * FROM logical_content_projection WHERE content_hash = 'h5'")
        .unwrap();
    let rows = statement
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows
}

// Without planner statistics SQLite chose `idx_paths_media_repair_by_id` for
// the ranking subqueries and read every live path for each projected content,
// on every path insert, update and delete.
#[test]
fn the_projection_ranks_copies_through_the_content_hash_index() {
    let dir = tempfile::tempdir().unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    for i in 0..2000 {
        let hash = format!("h{}", i / 2);
        if i % 2 == 0 {
            conn.execute("INSERT INTO contents (hash, byte_size, kind) VALUES (?1, 1, 'image')", [&hash]).unwrap();
        }
        conn.execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) VALUES (?1, '/d', ?2, 'image', ?3)",
            rusqlite::params![format!("/d/f{i}.jpg"), format!("f{i}.jpg"), hash],
        )
        .unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();

    let plan = projection_plan(&conn);
    let ranked: Vec<_> = plan.iter().filter(|step| step.contains("ranked")).collect();
    assert_eq!(ranked.len(), 3, "{plan:#?}");
    assert!(
        ranked.iter().all(|step| step.contains("idx_paths_content_hash (content_hash=?)")),
        "{plan:#?}"
    );
}

#[test]
fn an_index_with_an_older_projection_gets_the_current_one_and_an_ordinary_open_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite3");
    drop(index_store::open(&file).unwrap());
    {
        let conn = rusqlite::Connection::open(&file).unwrap();
        let current: String = conn
            .query_row("SELECT sql FROM sqlite_master WHERE name = 'logical_content_projection'", [], |row| row.get(0))
            .unwrap();
        let older = current.replace(" INDEXED BY idx_paths_content_hash", "");
        conn.execute_batch(&format!("DROP VIEW logical_content_projection; {older};")).unwrap();
    }

    let conn = index_store::open(&file).unwrap();
    let refreshed: String = conn
        .query_row("SELECT sql FROM sqlite_master WHERE name = 'logical_content_projection'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(refreshed.matches("INDEXED BY idx_paths_content_hash").count(), 3);
    let version = |conn: &rusqlite::Connection| -> i64 {
        conn.pragma_query_value(None, "schema_version", |row| row.get(0)).unwrap()
    };
    let before = version(&conn);
    drop(conn);
    let reopened = index_store::open(&file).unwrap();
    assert_eq!(version(&reopened), before, "an ordinary open replays no DDL");
}

// A temporary identity names a path id, and path ids start again after a
// rebuild empties `paths`: a result kept under one would belong to whichever
// file gets that id next.
#[test]
fn a_rebuild_never_keeps_results_under_a_temporary_identity() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO transcripts (content_hash, model, model_version, text, segments, created_at_utc)
         VALUES ('p17', 'm', 'v', 'someone else', '[]', 'now'), ('abc123', 'm', 'v', 'kept', '[]', 'now');
         INSERT INTO face_checks (content_hash, model, model_version, face_count, checked_at_utc)
         VALUES ('p17', 'm', 'v', 1, 'now'), ('abc123', 'm', 'v', 1, 'now');
         INSERT INTO faces (content_hash, x1, y1, x2, y2, confidence, model, model_version)
         VALUES ('p17', 0, 0, 1, 1, 0.9, 'm', 'v'), ('abc123', 0, 0, 1, 1, 0.9, 'm', 'v');",
    )
    .unwrap();

    index_store::clear_reconstructible(&conn, false, false).unwrap();

    for table in ["transcripts", "face_checks", "faces"] {
        let keys: Vec<String> = conn
            .prepare(&format!("SELECT content_hash FROM {table}"))
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(keys, vec!["abc123".to_string()], "{table}");
    }
}

use onecopy_lib::index_store;

#[test]
fn newer_unknown_schema_is_not_destructively_downgraded() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch("INSERT INTO contents (hash, byte_size, kind) VALUES ('retained', 1, 'image'); PRAGMA user_version = 999;").unwrap();
    drop(conn);
    assert!(index_store::open(&db).is_err());
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
        999
    );
}

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
         INSERT INTO recent_notifications
           (kind, path, level, presentation, message, first_seen_utc, last_seen_utc)
         VALUES ('read-error', '/photos/a.jpg', 'error', 'persistent', 'failed',
                 '2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z');
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

    index_store::clear_reconstructible(&conn, false, false).unwrap();
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
        "issues",
        "recent_notifications",
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


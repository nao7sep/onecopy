use onecopy_lib::index_store;
use rusqlite::OptionalExtension;

#[test]
fn offset_repair_marks_only_unknown_image_evidence_and_preserves_visibility() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    for (id, kind, source, offset_known, missing) in [
        (1, "image", "metadata", 0, 0),
        (2, "companion", "metadata", 0, 0),
        (3, "image", "metadata", 1, 0),
        (4, "video", "metadata", 0, 0),
        (5, "image", "filename", 0, 0),
        (6, "image", "metadata", 0, 1),
    ] {
        conn.execute("INSERT INTO paths(id, abs_path, dir_path, file_name, kind, indexed_at_utc, resolved_source, resolved_utc_ms, missing)
            VALUES (?1, ?2, '/root', ?3, ?4, 'checked', ?5, 1000, ?6)",
            rusqlite::params![id, format!("/root/{id}.tif"), format!("{id}.tif"), kind, source, missing]).unwrap();
        conn.execute("INSERT INTO evidence(path_id, source, raw, offset_known) VALUES (?1, ?2, 'retained evidence', ?3)",
            rusqlite::params![id, source, offset_known]).unwrap();
    }
    let policy = onecopy_lib::visibility::Policy::from_config(&serde_json::json!({
        "ignoredFileNames": ["1.tif"], "hideDotNames": false,
    })).unwrap();
    onecopy_lib::visibility_index::apply_policy(&conn, &policy).unwrap();
    let flags: i64 = conn.query_row("SELECT hidden_flags FROM visibility_policy", [], |row| row.get(0)).unwrap();
    conn.execute_batch("PRAGMA user_version = 13").unwrap();
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    let pending: Vec<i64> = conn.prepare("SELECT id FROM paths WHERE indexed_at_utc IS NULL ORDER BY id").unwrap()
        .query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(pending, vec![1, 2, 6]);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM evidence", [], |row| row.get::<_, i64>(0)).unwrap(), 6);
    assert_eq!(conn.query_row("SELECT review_visible FROM paths WHERE id = 1", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(conn.query_row("SELECT hidden_flags FROM visibility_policy", [], |row| row.get::<_, i64>(0)).unwrap(), flags);
    assert_eq!(conn.query_row("SELECT name FROM visibility_ignored_names", [], |row| row.get::<_, String>(0)).unwrap(), "1.tif");
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM logical_projection_batch", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    conn.execute("UPDATE paths SET indexed_at_utc = 'checked again' WHERE id = 1", []).unwrap();
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    assert_eq!(conn.query_row("SELECT indexed_at_utc FROM paths WHERE id = 1", [], |row| row.get::<_, String>(0)).unwrap(), "checked again");
}

#[test]
fn failed_offset_repair_rolls_back_version_evidence_and_projection_gate() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch("INSERT INTO paths(id, abs_path, dir_path, file_name, kind, indexed_at_utc, resolved_source)
          VALUES (1, '/root/a.tif', '/root', 'a.tif', 'image', 'checked', 'metadata');
        INSERT INTO evidence(path_id, source, raw, offset_known) VALUES (1, 'metadata', 'retained', 0);
        CREATE TRIGGER fail_offset_repair BEFORE UPDATE OF indexed_at_utc ON paths
          BEGIN SELECT RAISE(ABORT, 'injected repair failure'); END;
        PRAGMA user_version = 13;").unwrap();
    drop(conn);
    assert!(index_store::open(&db).is_err());
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)).unwrap(), 13);
    assert_eq!(conn.query_row("SELECT indexed_at_utc FROM paths", [], |row| row.get::<_, String>(0)).unwrap(), "checked");
    assert_eq!(conn.query_row("SELECT raw FROM evidence", [], |row| row.get::<_, String>(0)).unwrap(), "retained");
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM logical_projection_batch", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

fn revision_twelve_fixture(path: &std::path::Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.create_collation("onecopy_nocase", |left, right| left.to_lowercase().cmp(&right.to_lowercase())).unwrap();
    conn.execute_batch(include_str!("fixtures/index-v12.sql")).unwrap();
    conn
}

/// Seeds an `issues` row directly in the frozen v12 shape (no message_key or
/// message_values, which that revision never had), mirroring
/// `index_store::upsert_issue`'s coalesce-by-(kind, path) behavior. The real
/// `upsert_issue` now requires the current schema, so a connection pinned to
/// v12 seeds through this instead of a live migration.
fn seed_v12_issue(conn: &rusqlite::Connection, path: Option<&str>, kind: &str, message: &str) {
    let path = path.unwrap_or("");
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM issues WHERE kind = ?1 AND path = ?2 AND closed_at_utc IS NULL",
            rusqlite::params![kind, path],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE issues SET message = ?2, last_seen_utc = ?3, occurrence_count = occurrence_count + 1 WHERE id = ?1",
                rusqlite::params![id, message, "2026-01-01T00:00:00.000Z"],
            )
            .unwrap();
        }
        None => {
            conn.execute(
                "INSERT INTO issues (path, kind, message, first_seen_utc, last_seen_utc) VALUES (?1, ?2, ?3, ?4, ?4)",
                rusqlite::params![path, kind, message, "2026-01-01T00:00:00.000Z"],
            )
            .unwrap();
        }
    }
}

#[test]
fn revision_twelve_visibility_upgrade_preserves_history_caches_and_copy_evidence() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
    conn.execute_batch("INSERT INTO contents (hash, byte_size, kind, derived_at_utc) VALUES ('photo', 3, 'image', 'ready');
        INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_source, resolved_utc_ms)
          VALUES ('/root/a.jpg', '/root', 'a.jpg', 'image', 'photo', 'filename', 1000),
                 ('/root/b.jpg', '/root', 'b.jpg', 'image', 'photo', 'filename', 2000);").unwrap();
    seed_v12_issue(&conn, Some("/root/a.jpg"), "read-error", "retained");
    index_store::dismiss_issues(&conn, None).unwrap();
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    assert_eq!(conn.query_row("SELECT live_copy_count, visible_copy_count, resolved_utc_ms FROM logical_contents", [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))).unwrap(), (2, 2, 1000));
    assert_eq!(conn.query_row("SELECT derived_at_utc FROM contents", [], |row| row.get::<_, String>(0)).unwrap(), "ready");
    assert_eq!(conn.query_row("SELECT closure FROM issues", [], |row| row.get::<_, String>(0)).unwrap(), "dismissed");
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM paths WHERE visibility_checked = 0", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
}

fn restore_legacy_issue_shape(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "DROP VIEW active_issues;
         ALTER TABLE issues RENAME TO fixture_history_shape;
         DROP INDEX idx_issues_live_identity;
         DROP INDEX idx_issues_first_seen;
         CREATE TABLE issues (
           id INTEGER PRIMARY KEY, path TEXT NOT NULL DEFAULT '', kind TEXT NOT NULL, message TEXT,
           first_seen_utc TEXT NOT NULL, last_seen_utc TEXT NOT NULL,
           occurrence_count INTEGER NOT NULL DEFAULT 1, UNIQUE(kind, path)
         );
         INSERT INTO issues SELECT id, path, kind, message, first_seen_utc, last_seen_utc, occurrence_count FROM fixture_history_shape;
         DROP TABLE fixture_history_shape;
         CREATE INDEX idx_issues_first_seen ON issues(first_seen_utc, id);"
    ).unwrap();
}

#[test]
fn revision_nine_upgrade_preserves_library_and_diagnostics_across_concurrent_openers() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind, derived_at_utc) VALUES ('kept', 4, 'image', 'ready');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) VALUES ('/kept.jpg', '/', 'kept.jpg', 'image', 'kept');
         INSERT INTO issues (path, kind, message, first_seen_utc, last_seen_utc) VALUES ('/kept.jpg', 'read-error', 'diagnostic', '2026-09-09T00:00:00.000Z', '2026-09-09T00:00:00.000Z');
         INSERT INTO recent_notifications (kind, path, level, presentation, message, first_seen_utc, last_seen_utc)
         VALUES ('read-error', '/kept.jpg', 'error', 'persistent', 'retained', '2026-09-09T00:00:00.000Z', '2026-09-09T00:00:00.000Z');
         ALTER TABLE paths DROP COLUMN hash_attempt_failed;
         ALTER TABLE paths DROP COLUMN metadata_attempt_failed;
         PRAGMA user_version = 9;"
    ).unwrap();
    restore_legacy_issue_shape(&conn);
    drop(conn);
    let start = std::sync::Arc::new(std::sync::Barrier::new(4));
    let workers = (0..4)
        .map(|_| {
            let db = db.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                drop(index_store::open(&db).unwrap());
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    let conn = index_store::open(&db).unwrap();
    for table in [
        "contents",
        "paths",
        "logical_contents",
        "issues",
        "recent_notifications",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "{table}"
        );
    }
    assert_eq!(
        conn.query_row(
            "SELECT hash_attempt_failed + metadata_attempt_failed FROM paths",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT derived_at_utc FROM contents", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "ready"
    );
}

#[test]
fn revision_ten_upgrade_retains_issue_identity_and_allows_new_occurrences_after_dismissal() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
    seed_v12_issue(&conn, Some("/photo.jpg"), "read-error", "retained detail");
    seed_v12_issue(&conn, Some("/photo.jpg"), "read-error", "retained detail");
    restore_legacy_issue_shape(&conn);
    let before: (i64, String, String, i64) = conn
        .query_row(
            "SELECT id, first_seen_utc, last_seen_utc, occurrence_count FROM issues",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    conn.pragma_update(None, "user_version", 10).unwrap();
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    let after = conn
        .query_row(
            "SELECT id, first_seen_utc, last_seen_utc, occurrence_count FROM active_issues",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(before, after);
    index_store::dismiss_issues(&conn, Some(before.0)).unwrap();
    index_store::upsert_issue(&conn, Some("/photo.jpg"), "read-error", "new attempt").unwrap();
    assert_eq!(onecopy_lib::queries::issues(&conn, 10, None).unwrap().0, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn failed_history_upgrade_rolls_back_the_whole_migration() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
    seed_v12_issue(&conn, None, "read-error", "retained");
    restore_legacy_issue_shape(&conn);
    // A malformed old schema fails after the rename, while copying records.
    conn.execute_batch(
        "ALTER TABLE issues DROP COLUMN message;
        ALTER TABLE paths DROP COLUMN hash_attempt_failed;
        ALTER TABLE paths DROP COLUMN metadata_attempt_failed;
        PRAGMA user_version = 9;",
    )
    .unwrap();
    drop(conn);
    assert!(index_store::open(&db).is_err());
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('paths') WHERE name = 'hash_attempt_failed'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'issues_before_history'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn revision_eleven_preserves_archived_closures_and_accepts_new_attempt_reasons() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
    seed_v12_issue(&conn, None, "test", "dismissed detail");
    index_store::dismiss_issues(&conn, None).unwrap();
    seed_v12_issue(&conn, None, "test", "live detail");
    let old: (i64, String, String) = conn
        .query_row(
            "SELECT id, closed_at_utc, closure FROM issues WHERE closure IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    conn.execute_batch("PRAGMA user_version = 11;").unwrap();
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    let kept = conn
        .query_row(
            "SELECT id, closed_at_utc, closure FROM issues WHERE closure IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(old, kept);
    index_store::begin_issue_run(&conn).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM issues WHERE closure = 'app-restart'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM issues WHERE closure = 'dismissed'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

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
         VALUES ('/photos', 'now');",
    )
    .unwrap();

    index_store::clear_reconstructible(&conn).unwrap();

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
fn revision_fourteen_upgrade_splits_the_shared_empty_identity() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('empty', 0, 'other'), ('full', 4, 'other');
         INSERT INTO paths (id, abs_path, dir_path, file_name, kind, content_hash, size)
           VALUES (1, '/r/a.txt', '/r', 'a.txt', 'other', 'empty', 0),
                  (2, '/r/.gitkeep', '/r', '.gitkeep', 'other', 'empty', 0),
                  (3, '/r/c.txt', '/r', 'c.txt', 'other', 'full', 4);
         INSERT INTO evidence (content_hash, path_id, source, raw) VALUES ('empty', 1, 'filesystem', 'kept');
         PRAGMA user_version = 14;",
    )
    .unwrap();
    drop(conn);

    let conn = index_store::open(&db).unwrap();
    let count = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT COUNT(*) FROM paths WHERE content_hash IS NULL AND size = 0"), 2);
    assert_eq!(count("SELECT COUNT(*) FROM contents WHERE hash = 'empty'"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM logical_contents WHERE content_hash = 'empty'"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM logical_contents WHERE content_hash = 'full'"), 1);
    assert_eq!(count("SELECT COUNT(*) FROM evidence WHERE path_id = 1 AND content_hash IS NULL"), 1);
    assert_eq!(count("SELECT COUNT(*) FROM logical_projection_batch"), 0);
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

/// (R4.1 finding 3) A database upgraded from a pre-fix revision recomputes
/// every existing item's section from its representative copy rather than
/// keeping the stale `contents.kind`-derived value.
#[test]
fn revision_eighteen_upgrade_recomputes_kind_from_the_representative_copy() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 4, 'other');
         INSERT INTO paths (id, abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES
             (1, '/r/photo.jpg.bak', '/r', 'photo.jpg.bak', 'other', 'h1', 2000, 'metadata'),
             (2, '/r/photo.jpg', '/r', 'photo.jpg', 'image', 'h1', 1000, 'metadata');
         PRAGMA user_version = 17;",
    )
    .unwrap();
    // Force the stale, pre-fix projection row a revision-17 database would
    // still carry (the old view wrote 'other' here from contents.kind).
    conn.execute(
        "UPDATE logical_contents SET kind = 'other' WHERE content_hash = 'h1'",
        [],
    )
    .unwrap();
    drop(conn);

    let conn = index_store::open(&db).unwrap();
    let kind: String = conn
        .query_row(
            "SELECT kind FROM logical_contents WHERE content_hash = 'h1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "image");
}

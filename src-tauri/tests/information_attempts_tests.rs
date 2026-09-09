use onecopy_lib::{
    index_store, information_attempts as attempts, metadata, preview::CachePaths, scanner,
};

fn missing_file() -> (
    tempfile::TempDir,
    rusqlite::Connection,
    std::path::PathBuf,
    CachePaths,
) {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let path = root.path().join("photo.jpg");
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('known', 4, 'image')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO paths (abs_path, dir_path, file_name, kind, size) VALUES (?1, ?2, 'photo.jpg', 'image', 4)",
        rusqlite::params![path.to_string_lossy(), root.path().to_string_lossy()]).unwrap();
    let cache = CachePaths::new(root.path().join("cache"));
    (root, conn, path, cache)
}

#[test]
fn io_failure_is_not_successful_empty_metadata() {
    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("absent.jpg");
    assert_eq!(
        metadata::read_image_metadata(&absent).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(
        metadata::read_video_metadata(&absent).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    let no_metadata = root.path().join("plain.jpg");
    std::fs::write(&no_metadata, b"this is not an EXIF container").unwrap();
    assert!(metadata::read_image_metadata(&no_metadata)
        .unwrap()
        .taken
        .is_none());
    assert!(metadata::read_video_metadata(&no_metadata)
        .unwrap()
        .taken
        .is_none());
}

#[test]
fn independent_read_failures_settle_without_completing_information_or_repeating() {
    let (_root, conn, _path, cache) = missing_file();
    assert_eq!(scanner::hash_pending(&conn, &cache).unwrap().errors, 1);
    assert_eq!(scanner::extract_pending(&conn).unwrap().failed, 1);
    let state: (i64, i64, Option<String>) = conn
        .query_row(
            "SELECT hash_attempt_failed, metadata_attempt_failed, indexed_at_utc FROM paths",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, (1, 1, None));
    assert!(!scanner::pending_index_work_exists(&conn).unwrap());
    for _ in 0..3 {
        assert_eq!(scanner::hash_pending(&conn, &cache).unwrap().errors, 0);
        assert_eq!(scanner::extract_pending(&conn).unwrap().failed, 0);
    }
    assert_eq!(
        conn.query_row("SELECT SUM(occurrence_count) FROM issues", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn section_recheck_reopens_both_stages_and_success_clears_the_condition() {
    let (_root, conn, path, cache) = missing_file();
    scanner::hash_pending(&conn, &cache).unwrap();
    scanner::extract_pending(&conn).unwrap();
    std::fs::write(&path, b"data").unwrap();
    assert_eq!(scanner::hash_pending(&conn, &cache).unwrap().full_hashed, 0);
    assert_eq!(scanner::extract_pending(&conn).unwrap().extracted, 0);
    assert_eq!(attempts::reset_section(&conn, "image", None).unwrap(), 1);
    assert!(scanner::pending_index_work_exists(&conn).unwrap());
    assert_eq!(scanner::hash_pending(&conn, &cache).unwrap().full_hashed, 1);
    assert_eq!(scanner::extract_pending(&conn).unwrap().extracted, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn database_reopen_does_not_retry_but_launch_boundary_does() {
    let (root, conn, _path, cache) = missing_file();
    scanner::hash_pending(&conn, &cache).unwrap();
    scanner::extract_pending(&conn).unwrap();
    drop(conn);
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    assert!(!scanner::pending_index_work_exists(&conn).unwrap());
    assert_eq!(attempts::reset_library(&conn).unwrap(), 1);
    assert_eq!(scanner::hash_pending(&conn, &cache).unwrap().errors, 1);
    assert_eq!(scanner::extract_pending(&conn).unwrap().failed, 1);
    assert!(!scanner::pending_index_work_exists(&conn).unwrap());
}

#[test]
fn recheck_scope_does_not_reset_a_neighboring_kind_or_month_in_the_same_folder() {
    let (_root, conn, _path, _cache) = missing_file();
    conn.execute_batch(
        "UPDATE paths SET hash_attempt_failed = 1, metadata_attempt_failed = 1;
         INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, hash_attempt_failed, metadata_attempt_failed)
         VALUES ('/other.jpg', '/', 'other.jpg', 'image', 200, 1, 1),
                ('/movie.mp4', '/', 'movie.mp4', 'video', NULL, 1, 1);"
    ).unwrap();
    assert_eq!(attempts::reset_section(&conn, "image", None).unwrap(), 1);
    assert_eq!(
        conn.query_row("SELECT SUM(hash_attempt_failed) FROM paths", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        2
    );
    assert_eq!(
        attempts::reset_section(&conn, "image", Some((100, 200))).unwrap(),
        0
    );
    assert_eq!(
        attempts::reset_section(&conn, "image", Some((200, 300))).unwrap(),
        1
    );
}

#[test]
fn a_companions_failed_metadata_follows_its_main_files_section() {
    let (_root, conn, _path, _cache) = missing_file();
    conn.execute_batch(
        "UPDATE paths SET content_hash = 'known', indexed_at_utc = '2026-09-09T00:00:00.000Z', resolved_utc_ms = 200, resolved_source = 'filename';
         INSERT INTO paths (abs_path, dir_path, file_name, kind, companion_of, metadata_attempt_failed)
         SELECT '/photo.raw', '/', 'photo.raw', 'companion', id, 1 FROM paths;
         INSERT INTO paths (abs_path, dir_path, file_name, kind, metadata_attempt_failed)
         VALUES ('/unpaired.raw', '/', 'unpaired.raw', 'companion', 1);"
    ).unwrap();
    assert_eq!(attempts::reset_section(&conn, "image", None).unwrap(), 0);
    assert_eq!(
        attempts::reset_section(&conn, "image", Some((200, 300))).unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT metadata_attempt_failed FROM paths WHERE abs_path = '/unpaired.raw'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn failed_live_photo_backfill_does_not_claim_success_or_repeat() {
    let (_root, conn, _path, _cache) = missing_file();
    conn.execute(
        "UPDATE paths SET indexed_at_utc = '2026-09-09T00:00:00.000Z', resolved_source = 'undated', content_hash = 'known'",
        [],
    )
    .unwrap();
    assert_eq!(scanner::extract_pending(&conn).unwrap().failed, 1);
    assert_eq!(scanner::extract_pending(&conn).unwrap().failed, 0);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM evidence WHERE source = 'live-photo-identifier'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert!(!scanner::pending_index_work_exists(&conn).unwrap());
}

#[test]
fn a_recording_failure_does_not_silently_settle_the_input() {
    let (_root, conn, path, _cache) = missing_file();
    conn.execute_batch("CREATE TRIGGER reject_issue BEFORE INSERT ON issues BEGIN SELECT RAISE(ABORT, 'fixture recording failure'); END;").unwrap();
    let id = conn
        .query_row("SELECT id FROM paths", [], |row| row.get::<_, i64>(0))
        .unwrap();
    assert!(attempts::failed(
        &conn,
        id,
        path.to_str().unwrap(),
        attempts::Stage::Identity,
        "unreadable"
    )
    .is_err());
    assert_eq!(
        conn.query_row("SELECT hash_attempt_failed FROM paths", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn source_change_reopens_receipts_but_unchanged_stat_does_not() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let path = root.path().join("photo.jpg");
    let lists = scanner::ScanLists {
        images: vec!["jpg".into()],
        videos: vec![],
        audio: vec![],
        companions: vec![],
    };
    std::fs::write(&path, b"data").unwrap();
    scanner::upsert_file(&conn, &path, &lists).unwrap();
    conn.execute(
        "UPDATE paths SET hash_attempt_failed = 1, metadata_attempt_failed = 1",
        [],
    )
    .unwrap();
    scanner::upsert_file(&conn, &path, &lists).unwrap();
    assert_eq!(
        conn.query_row("SELECT hash_attempt_failed FROM paths", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    std::fs::write(&path, b"changed bytes").unwrap();
    scanner::upsert_file(&conn, &path, &lists).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT hash_attempt_failed + metadata_attempt_failed FROM paths",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

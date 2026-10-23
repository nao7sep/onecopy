use super::{
    directory_belongs_to_root, live_images_sql, live_photo_pair_candidates_sql, raw_pair_candidates_sql, scan_issue_message_key,
    COPIES_DISAGREE, METADATA_READ_ERROR, READ_ERROR, STAT_ERROR, WALK_ERROR,
};

#[test]
fn directory_containment_uses_native_components_and_spelling_aliases() {
    for (directory, root) in [
        ("/Photos/Nested", "/Photos"),
        ("/Photos", "/Photos/"),
        ("/Photos/Nested", "/"),
    ] {
        assert!(directory_belongs_to_root(directory, root), "{directory} under {root}");
    }
    for (directory, root) in [
        ("/Photos2/Nested", "/Photos"),
    ] {
        assert!(!directory_belongs_to_root(directory, root), "{directory} is a different root from {root}");
    }
    #[cfg(not(windows))]
    {
        assert!(!directory_belongs_to_root("/photos/Nested", "/Photos"));
        assert!(!directory_belongs_to_root("/Photos/Private", r"/Photos\Private"));
        assert!(directory_belongs_to_root(r"/Photos\Private/Nested", r"/Photos\Private"));
    }
    #[cfg(windows)]
    for (directory, root) in [
        (r"\\?\c:\PHOTOS\Nested", r"C:/Photos"),
        (r"c:\photos\Nested", r"\\?\C:\Photos"),
        (r"\\?\UNC\server\share\PHOTOS\Nested", r"\\server\share\Photos"),
    ] {
        assert!(directory_belongs_to_root(directory, root), "{directory} under {root}");
    }
}

#[test]
fn every_scan_issue_message_key_exists_in_every_embedded_catalogue() {
    // A typo here would fall back to `t()`'s key-echo behavior in every
    // language at once, for every scan-time Issue OneCopy raises.
    let kinds = [WALK_ERROR, STAT_ERROR, READ_ERROR, METADATA_READ_ERROR, COPIES_DISAGREE, "unmapped-kind"];
    for language in crate::i18n::LANGUAGES {
        let text = crate::i18n::catalogue(language);
        for kind in kinds {
            let key = scan_issue_message_key(kind);
            assert!(text.has(key), "{language} lacks {key} (from kind {kind})");
        }
    }
}

fn plan(conn: &rusqlite::Connection, sql: String) -> Vec<String> {
    let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn pairing_tables(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "CREATE TEMP TABLE onecopy_pair_scope (
           dir_path TEXT PRIMARY KEY
         ) WITHOUT ROWID;
         CREATE TEMP TABLE onecopy_pair_results (
           path_id INTEGER PRIMARY KEY,
           primary_id INTEGER
         ) WITHOUT ROWID;
         CREATE TEMP TABLE onecopy_live_images (
           dir_path TEXT NOT NULL,
           raw TEXT NOT NULL,
           image_id INTEGER NOT NULL,
           PRIMARY KEY (dir_path, raw, image_id)
         ) WITHOUT ROWID;",
    )
    .unwrap();
}

#[test]
fn scoped_candidate_queries_seek_directories_before_relationship_facts() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    pairing_tables(&conn);

    let raw = plan(&conn, raw_pair_candidates_sql(true)).join("\n");
    assert!(raw.contains("idx_paths_pairing (dir_path=?)"), "{raw}");
    assert!(
        raw.contains("idx_paths_pairing (dir_path=? AND stem=?)"),
        "{raw}"
    );

    let images = plan(&conn, live_images_sql(true)).join("\n");
    assert!(images.contains("idx_paths_dir (dir_path=?)"), "{images}");
    assert!(!images.contains("idx_evidence_source_raw"), "{images}");
}

#[test]
fn a_movie_finds_its_still_with_one_keyed_lookup() {
    // Scanning the movie's folder for each movie is quadratic in folder size,
    // and an identifier lookup across the whole index is quadratic in copies
    // of a folder; both pairing queries seek the collected stills by folder
    // and identifier instead.
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    pairing_tables(&conn);
    for scoped in [false, true] {
        for sql in [raw_pair_candidates_sql(scoped), live_photo_pair_candidates_sql(scoped)] {
            let plan = plan(&conn, sql).join("\n");
            assert!(plan.contains("live USING PRIMARY KEY (dir_path=? AND raw=?)"), "{plan}");
            assert!(!plan.contains("idx_paths_dir"), "{plan}");
            assert!(!plan.contains("idx_evidence_source_raw"), "{plan}");
        }
    }
}

#[cfg(unix)]
#[test]
fn native_names_remain_distinct_in_inventory_pairing_and_readers() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let fixture = dir.path().join("metadata.txt");
    std::fs::write(&fixture, b"contents").unwrap();
    let metadata = std::fs::metadata(&fixture).unwrap();
    let lists = super::settings_from_config(None, dir.path(), 0).lists;
    let invalid = dir.path().join(std::ffi::OsString::from_vec(b"shot-\xff.JPG".to_vec()));
    let other_invalid = dir.path().join(std::ffi::OsString::from_vec(b"shot-\xfe.JPG".to_vec()));
    let replacement = dir.path().join("shot-\u{fffd}.JPG");
    let literal = dir.path().join("shot-\\bff.JPG");
    for path in [&invalid, &other_invalid, &replacement, &literal] {
        assert_eq!(super::upsert_file(&conn, path, &metadata, &lists, 0).unwrap(), super::Upsert::Added);
        let encoded = crate::native_path::encode(path);
        let (id, name, stem, ext): (i64, String, String, String) = conn.query_row(
            "SELECT id, file_name, stem, ext FROM paths WHERE abs_path = ?1", [&encoded],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(name, crate::native_path::encode(path.file_name().unwrap()));
        assert_eq!(stem, crate::native_path::encode(path.file_stem().unwrap()).to_lowercase());
        assert_eq!(ext, "jpg");
        let reopened = crate::indexed_file::live_path_with(&conn, None, Some(id), &|candidate| candidate == path).unwrap();
        assert_eq!(&reopened, path);
    }
    assert_eq!(conn.query_row("SELECT COUNT(DISTINCT stem) FROM paths", [], |row| row.get::<_, i64>(0)).unwrap(), 4);
}

#[cfg(unix)]
#[test]
fn descendant_scopes_compare_encoded_bytes_past_nul() {
    use std::os::unix::ffi::OsStringExt;
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE candidates(path TEXT);").unwrap();
    let root = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/photos/\xff".to_vec()));
    let peer = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"/photos/\xfe".to_vec()));
    let candidates = [root.join("inside.jpg"), root.with_extension("sibling").join("outside.jpg"), peer.join("outside.jpg"), std::path::PathBuf::from("/photos/\u{fffd}/outside.jpg")];
    for path in &candidates {
        conn.execute("INSERT INTO candidates VALUES (?1)", [crate::native_path::encode(path)]).unwrap();
    }
    let prefix = super::ensure_trailing_separator(&crate::native_path::encode(&root));
    let sql = format!("SELECT path FROM candidates WHERE {}", super::descendant_path_predicate("path", 1));
    let mut statement = conn.prepare(&sql).unwrap();
    let matches: Vec<String> = statement.query_map([prefix], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(matches, vec![crate::native_path::encode(&candidates[0]).into_owned()]);
    assert!(super::directory_belongs_to_root(&crate::native_path::encode(&candidates[0]), &crate::native_path::encode(&root)));
    assert!(!super::directory_belongs_to_root(&crate::native_path::encode(&candidates[2]), &crate::native_path::encode(&root)));
}

#[cfg(unix)]
#[test]
fn native_filesystem_inventory_reopens_an_invalid_byte_name_when_supported() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(std::ffi::OsString::from_vec(b"native-\xff.txt".to_vec()));
    if let Err(error) = std::fs::write(&path, b"native contents") {
        // APFS rejects non-Unicode names; all other filesystem failures must fail.
        if error.kind() == std::io::ErrorKind::InvalidInput || (cfg!(target_os = "macos") && error.raw_os_error() == Some(92)) {
            eprintln!("SKIP native filesystem inventory: {error}");
            return;
        }
        panic!("create native fixture: {error}");
    }
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let lists = super::settings_from_config(None, dir.path(), 0).lists;
    super::upsert_file(&conn, &path, &std::fs::metadata(&path).unwrap(), &lists, 0).unwrap();
    let id = conn.query_row("SELECT id FROM paths", [], |row| row.get::<_, i64>(0)).unwrap();
    let reopened = crate::indexed_file::live_path(&conn, None, Some(id)).unwrap();
    assert_eq!(reopened, path);
    assert_eq!(std::fs::read(reopened).unwrap(), b"native contents");
}

#[cfg(unix)]
#[test]
fn discovery_keeps_case_only_siblings_of_the_data_root() {
    use super::is_excluded_from_discovery;
    use std::path::Path;
    let root = Path::new("/archive/onecopy");
    assert!(is_excluded_from_discovery(Path::new("/archive/onecopy/cache/image.jpg"), false, Some(root)));
    assert!(!is_excluded_from_discovery(Path::new("/archive/OneCopy"), true, Some(root)));
    assert!(!is_excluded_from_discovery(Path::new("/archive/OneCopy/image.jpg"), false, Some(root)));
}

#[test]
fn extraction_reads_only_a_page_and_leaves_new_arrivals_for_the_next_pass() {
    use rusqlite::params;
    let _serial = crate::scan_runtime::serial_test();
    super::SCAN_CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
    let temp = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let total = 2 * super::EXTRACT_PAGE_SIZE as i64 + 5;
    for id in 1..=total {
        conn.execute(
            "INSERT INTO paths(id, abs_path, dir_path, file_name, kind) VALUES (?1, ?2, '/fixture', ?3, 'other')",
            params![id, format!("/fixture/{id}.txt"), format!("{id}.txt")],
        ).unwrap();
    }
    let next_page = super::EXTRACT_PAGE_SIZE as i64 + 1;
    // A deliberately unreadable SQL type detects eager string materialization
    // beyond the page. It becomes a valid current row after processing starts.
    conn.execute("UPDATE paths SET abs_path = CAST(abs_path AS BLOB) WHERE id = ?1", [next_page]).unwrap();
    let changed = std::cell::Cell::new(false);
    let stats = super::extract_pending_with_progress(&conn, &|progress| {
        if progress.done == 1 && !changed.replace(true) {
            conn.execute("UPDATE paths SET abs_path = CAST(abs_path AS TEXT), file_name = '20260101.txt' WHERE id = ?1", [next_page]).unwrap();
            conn.execute("DELETE FROM paths WHERE id = ?1", [next_page + 1]).unwrap();
            conn.execute("UPDATE paths SET missing = 1 WHERE id = ?1", [next_page + 2]).unwrap();
            conn.execute("INSERT INTO paths(id, abs_path, dir_path, file_name, kind) VALUES (?1, '/fixture/new.txt', '/fixture', 'new.txt', 'other')", [total + 1]).unwrap();
        }
    }).unwrap();
    assert!(changed.get());
    assert_eq!(stats.extracted, (total - 2) as u64);
    assert_eq!(stats.failed, 0);
    assert_eq!(conn.query_row("SELECT count(*) FROM evidence WHERE path_id = ?1 AND source = 'filename'", [next_page], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert!(conn.query_row("SELECT indexed_at_utc IS NULL FROM paths WHERE id = ?1", [total + 1], |row| row.get::<_, bool>(0)).unwrap());
    assert!(conn.query_row("SELECT indexed_at_utc IS NULL FROM paths WHERE id = ?1", [next_page + 2], |row| row.get::<_, bool>(0)).unwrap());
    assert_eq!(super::extract_pending(&conn).unwrap().extracted, 1);
}

#[test]
fn cancelling_extraction_leaves_remaining_pages_eligible_for_resume() {
    let _serial = crate::scan_runtime::serial_test();
    super::SCAN_CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
    let temp = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let total = super::EXTRACT_PAGE_SIZE as i64 + 3;
    for id in 1..=total {
        conn.execute("INSERT INTO paths(id, abs_path, dir_path, file_name, kind) VALUES (?1, ?2, '/fixture', 'item.txt', 'other')", rusqlite::params![id, format!("/fixture/{id}.txt")]).unwrap();
    }
    let result = super::extract_pending_with_progress(&conn, &|progress| {
        if progress.done == 1 { super::SCAN_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst); }
    });
    super::SCAN_CANCEL.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(result.unwrap_err(), super::CANCELLED);
    assert_eq!(conn.query_row("SELECT count(*) FROM paths WHERE indexed_at_utc IS NOT NULL", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(super::extract_pending(&conn).unwrap().extracted, (total - 1) as u64);
}

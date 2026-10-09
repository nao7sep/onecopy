use super::{
    live_images_sql, live_photo_pair_candidates_sql, raw_pair_candidates_sql, scan_issue_message_key,
    COPIES_DISAGREE, METADATA_READ_ERROR, READ_ERROR, STAT_ERROR, WALK_ERROR,
};

#[test]
fn every_scan_issue_message_key_exists_in_every_embedded_catalogue() {
    // A typo here would fall back to `t()`'s key-echo behavior in every
    // language at once, for every scan-time Issue OneCopy raises (R5.5
    // D-L12, D-L13).
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

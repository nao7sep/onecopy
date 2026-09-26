use super::{
    live_photo_pair_candidates_sql, raw_pair_candidates_sql, scan_issue_message_key,
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

#[test]
fn scoped_candidate_queries_seek_directories_before_relationship_facts() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "CREATE TEMP TABLE onecopy_pair_scope (
           dir_path TEXT PRIMARY KEY
         ) WITHOUT ROWID;
         CREATE TEMP TABLE onecopy_pair_results (
           path_id INTEGER PRIMARY KEY,
           primary_id INTEGER
         ) WITHOUT ROWID;",
    )
    .unwrap();

    let raw = plan(&conn, raw_pair_candidates_sql(true)).join("\n");
    assert!(raw.contains("idx_paths_pairing (dir_path=?)"), "{raw}");
    assert!(
        raw.contains("idx_paths_pairing (dir_path=? AND stem=?)"),
        "{raw}"
    );

    let live = plan(&conn, live_photo_pair_candidates_sql(true)).join("\n");
    assert!(live.contains("idx_paths_pairing (dir_path=?)"), "{live}");
    assert!(live.contains("idx_paths_dir (dir_path=?)"), "{live}");
    assert_eq!(
        live.matches("idx_evidence_path (path_id=?)").count(),
        2,
        "{live}"
    );
    assert!(
        !live.contains("idx_evidence_source_raw"),
        "a global identifier lookup must never drive local pairing:\n{live}"
    );
}

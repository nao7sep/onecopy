use super::*;

#[test]
fn replacement_failure_preserves_completed_receipt_and_records_the_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('media', 10, 'video');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
         VALUES ('/media.mov', '/', 'media.mov', 'video', 'media');
         INSERT INTO analysis_receipts (content_hash, transcript_state)
         VALUES ('media', 'ready-text');",
    )
    .unwrap();

    record_transcript_replacement_failure(&conn, "/media.mov", "replacement failed").unwrap();

    let receipt: String = conn
        .query_row(
            "SELECT transcript_state FROM analysis_receipts WHERE content_hash = 'media'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let issue: (String, String, String) = conn
        .query_row(
            "SELECT kind, message, message_key FROM issues WHERE path = '/media.mov'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(receipt, READY_TEXT);
    // The raw diagnostic stays as recorded detail; OneCopy's own sentence
    // follows the interface language through its catalogue key instead of
    // freezing as English at record time (R5.5 D-L12, D-L13).
    assert_eq!(
        issue,
        (
            TRANSCRIPT_ERROR.to_string(),
            "replacement failed".to_string(),
            derived_issue_message_key(TRANSCRIPT_ERROR).to_string()
        )
    );
}

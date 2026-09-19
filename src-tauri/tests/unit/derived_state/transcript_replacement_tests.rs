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
    let issue: (String, String) = conn
        .query_row(
            "SELECT kind, message FROM issues WHERE path = '/media.mov'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(receipt, READY_TEXT);
    assert_eq!(
        issue,
        (
            TRANSCRIPT_ERROR.to_string(),
            derived_issue_presentation(TRANSCRIPT_ERROR).to_string()
        )
    );
}

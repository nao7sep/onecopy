// Fixed-class output receipts and the safe Issues recovery boundary.

use onecopy_lib::preview::CachePaths;
use onecopy_lib::{derived_state, index_store, queries};

fn seeded() -> (tempfile::TempDir, rusqlite::Connection) {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derived-state-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents
           (hash, byte_size, kind, derived_at_utc, strip_frames)
         VALUES ('image', 1, 'image', 'failed', NULL),
                ('poster', 1, 'video', 'failed', NULL),
                ('strip', 1, 'video', 'ready', -1),
                ('face', 1, 'image', 'ready', NULL),
                ('speech', 1, 'video', 'ready', NULL),
                ('delete', 1, 'image', 'ready', NULL);
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
         VALUES ('/image.jpg', '/', 'image.jpg', 'image', 'image'),
                ('/poster.mov', '/', 'poster.mov', 'video', 'poster'),
                ('/strip.mov', '/', 'strip.mov', 'video', 'strip'),
                ('/face.jpg', '/', 'face.jpg', 'image', 'face'),
                ('/speech.mov', '/', 'speech.mov', 'video', 'speech'),
                ('/delete.jpg', '/', 'delete.jpg', 'image', 'delete');",
    )
    .unwrap();
    derived_state::record_face_failure(&conn, "face", "/face.jpg", "out of memory").unwrap();
    derived_state::record_transcript_failure(&conn, "speech", "/speech.mov", "decoder failed")
        .unwrap();
    for (path, kind) in [
        ("/image.jpg", "decode-error"),
        ("/poster.mov", derived_state::VIDEO_POSTER_ERROR),
        ("/strip.mov", derived_state::VIDEO_STRIP_ERROR),
        ("/delete.jpg", "delete-error"),
    ] {
        index_store::upsert_issue(&conn, Some(path), kind, "failed").unwrap();
    }
    (dir, conn)
}

#[test]
fn only_reconstructible_failures_offer_retry_and_stay_visible_while_queued() {
    let (_dir, conn) = seeded();
    let (_, rows) = queries::issues(&conn, 20).unwrap();
    assert_eq!(rows.len(), 6);
    assert!(rows
        .iter()
        .filter(|row| row.kind != "delete-error")
        .all(|row| row.recovery.as_ref().map(|action| action.status) == Some("available")));
    assert!(rows
        .iter()
        .find(|row| row.kind == "delete-error")
        .unwrap()
        .recovery
        .is_none());

    let face = rows
        .iter()
        .find(|row| row.kind == derived_state::FACE_ERROR)
        .unwrap();
    assert!(derived_state::retry_issue(&conn, face.id).unwrap());

    let (total, rows) = queries::issues(&conn, 20).unwrap();
    assert_eq!(total, 6, "retry never dismisses the evidence early");
    let face = rows
        .iter()
        .find(|row| row.kind == derived_state::FACE_ERROR)
        .unwrap();
    assert_eq!(face.recovery.as_ref().unwrap().status, "queued");
}

#[test]
fn retry_all_resets_each_safe_output_without_replaying_destructive_intent() {
    let (_dir, conn) = seeded();
    assert_eq!(derived_state::retry_all(&conn).unwrap(), 5);
    assert_eq!(
        derived_state::retry_all(&conn).unwrap(),
        0,
        "already queued outputs are deduplicated"
    );

    let derived: (Option<String>, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT
               (SELECT derived_at_utc FROM contents WHERE hash = 'image'),
               (SELECT derived_at_utc FROM contents WHERE hash = 'poster'),
               (SELECT strip_frames FROM contents WHERE hash = 'strip')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(derived, (None, None, None));
    let face_state: Option<String> = conn
        .query_row(
            "SELECT face_state FROM analysis_receipts WHERE content_hash = 'face'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(face_state, None);
    let transcript: Option<String> = conn
        .query_row(
            "SELECT transcript_state FROM analysis_receipts
             WHERE content_hash = 'speech'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(transcript, None);

    let (_, rows) = queries::issues(&conn, 20).unwrap();
    assert!(rows
        .iter()
        .find(|row| row.kind == "delete-error")
        .unwrap()
        .recovery
        .is_none());
}

#[test]
fn successful_analysis_records_value_or_empty_and_retires_its_issue() {
    let (_dir, conn) = seeded();
    derived_state::record_face_success(&conn, "face", "/face.jpg", 0.0).unwrap();
    derived_state::record_transcript_success(&conn, "speech", "/speech.mov", false).unwrap();

    let receipt: (String, String) = conn
        .query_row(
            "SELECT a.face_state, b.transcript_state FROM analysis_receipts a
             JOIN analysis_receipts b ON b.content_hash = 'speech'
             WHERE a.content_hash = 'face'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(receipt, ("ready".to_string(), "ready-empty".to_string()));
    let (_, rows) = queries::issues(&conn, 20).unwrap();
    assert!(!rows.iter().any(|row| matches!(
        row.kind.as_str(),
        derived_state::FACE_ERROR | derived_state::TRANSCRIPT_ERROR
    )));
}

#[test]
fn transcript_reads_distinguish_pending_failed_empty_and_missing_output() {
    let (dir, conn) = seeded();
    let cache = CachePaths::new(dir.path().join("cache"));

    let failed = derived_state::transcript_result(&conn, &cache, "speech").unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.message.as_deref(), Some("decoder failed"));

    let pending = derived_state::transcript_result(&conn, &cache, "poster").unwrap();
    assert_eq!(pending.status, "pending");

    let legacy = cache.transcript("poster");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "[0:01] kept\n").unwrap();
    let adopted = derived_state::transcript_result(&conn, &cache, "poster").unwrap();
    assert_eq!(adopted.status, "ready");
    assert_eq!(adopted.text.as_deref(), Some("[0:01] kept\n"));

    let target = cache.transcript("speech");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "").unwrap();
    derived_state::record_transcript_success(&conn, "speech", "/speech.mov", false).unwrap();
    let empty = derived_state::transcript_result(&conn, &cache, "speech").unwrap();
    assert_eq!(empty.status, "ready");
    assert_eq!(empty.text.as_deref(), Some(""));

    std::fs::remove_file(target).unwrap();
    let repaired = derived_state::transcript_result(&conn, &cache, "speech").unwrap();
    assert_eq!(repaired.status, "pending");
    let state: Option<String> = conn
        .query_row(
            "SELECT transcript_state FROM analysis_receipts WHERE content_hash = 'speech'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, None);
}

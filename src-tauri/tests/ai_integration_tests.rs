#![cfg(feature = "ai-test-engine")]

//! Fast AI integration over deterministic engines. These tests cross the
//! production receipt, cache-publication, retry, cancellation, and persisted
//! acceleration boundaries without managed artifacts or network access.

use onecopy_lib::ai_test_engine::{self, Outcome, Scenario};
use onecopy_lib::{ai_acceleration, derived_state, index_store, preview};
use rusqlite::params;

fn insert(conn: &rusqlite::Connection, hash: &str, kind: &str, path: &str) {
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind, derived_at_utc, derived_version)
         VALUES (?1, 1, ?2, 'ready', 3)",
        params![hash, kind],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing)
         VALUES (?1, '/', ?1, ?2, ?3, 0)",
        params![path, kind, hash],
    )
    .unwrap();
}

fn success(text: &str) -> Scenario {
    Scenario {
        outcome: Outcome::Success(text.to_string()),
        progress: vec![0, 25, 100],
        delay_ms: 0,
        cancel_at: None,
    }
}

#[test]
fn audio_and_video_publish_independent_durable_transcripts() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    insert(&conn, "audio", "audio", "voice.flac");
    insert(&conn, "video", "video", "clip.mp4");
    let cache = preview::CachePaths::new(root.path().join("cache"));

    for (hash, path) in [("audio", "voice.flac"), ("video", "clip.mp4")] {
        let result = ai_test_engine::transcribe(
            &conn,
            &cache,
            hash,
            path,
            false,
            &success("[0:00] canonical speech\n"),
        )
        .unwrap();
        assert_eq!(result.progress, [0, 25, 100]);
        assert_eq!(
            std::fs::read_to_string(cache.transcript(hash)).unwrap(),
            "[0:00] canonical speech\n"
        );
    }
    drop(conn);

    let reopened = index_store::open(&db).unwrap();
    for hash in ["audio", "video"] {
        assert_eq!(
            derived_state::transcript_result(&reopened, &cache, hash)
                .unwrap()
                .status,
            derived_state::READY
        );
    }
}

#[test]
fn empty_failure_cancellation_and_replacement_never_publish_partial_text() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let cache = preview::CachePaths::new(root.path().join("cache"));
    for hash in ["empty", "failed", "cancelled", "replacement"] {
        insert(&conn, hash, "audio", &format!("{hash}.flac"));
    }

    ai_test_engine::transcribe(
        &conn,
        &cache,
        "empty",
        "empty.flac",
        false,
        &Scenario {
            outcome: Outcome::Empty,
            progress: vec![100],
            delay_ms: 0,
            cancel_at: None,
        },
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(cache.transcript("empty")).unwrap(),
        ""
    );

    let failure = Scenario {
        outcome: Outcome::Failure("deterministic failure".to_string()),
        progress: vec![0, 50],
        delay_ms: 0,
        cancel_at: None,
    };
    assert!(
        ai_test_engine::transcribe(&conn, &cache, "failed", "failed.flac", false, &failure)
            .is_err()
    );
    assert!(!cache.transcript("failed").exists());

    let cancelled = Scenario {
        outcome: Outcome::Success("partial words".to_string()),
        progress: vec![0, 25, 100],
        delay_ms: 1,
        cancel_at: Some(1),
    };
    assert_eq!(
        ai_test_engine::transcribe(
            &conn,
            &cache,
            "cancelled",
            "cancelled.flac",
            false,
            &cancelled,
        )
        .unwrap_err(),
        onecopy_lib::scanner::CANCELLED
    );
    assert!(!cache.transcript("cancelled").exists());

    ai_test_engine::transcribe(
        &conn,
        &cache,
        "replacement",
        "replacement.flac",
        false,
        &success("[0:00] retained result\n"),
    )
    .unwrap();
    assert!(ai_test_engine::transcribe(
        &conn,
        &cache,
        "replacement",
        "replacement.flac",
        true,
        &failure,
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(cache.transcript("replacement")).unwrap(),
        "[0:00] retained result\n"
    );
}

#[test]
fn face_success_empty_and_failure_use_the_production_receipt_owner() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    for hash in ["smile", "none", "failed"] {
        insert(&conn, hash, "image", &format!("{hash}.jpg"));
    }
    ai_test_engine::score_face(&conn, "smile", "smile.jpg", &success("0.75")).unwrap();
    ai_test_engine::score_face(
        &conn,
        "none",
        "none.jpg",
        &Scenario {
            outcome: Outcome::Empty,
            progress: vec![100],
            delay_ms: 0,
            cancel_at: None,
        },
    )
    .unwrap();
    assert!(ai_test_engine::score_face(
        &conn,
        "failed",
        "failed.jpg",
        &Scenario {
            outcome: Outcome::Failure("detector failed".to_string()),
            progress: vec![0],
            delay_ms: 0,
            cancel_at: None,
        },
    )
    .is_err());
    let scores: (f64, f64) = conn
        .query_row(
            "SELECT a.face_score, b.face_score FROM contents a, contents b
             WHERE a.hash = 'smile' AND b.hash = 'none'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(scores, (0.75, 0.0));
    let failed: String = conn
        .query_row(
            "SELECT face_state FROM analysis_receipts WHERE content_hash = 'failed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failed, derived_state::FAILED);
}

#[test]
fn persisted_acceleration_is_explicit_and_unsupported_values_fail() {
    let cpu = serde_json::json!({
        "aiAcceleration": { "transcription": "none", "face-scoring": "none" }
    });
    let selected = ai_acceleration::selection_from_config(Some(&cpu)).unwrap();
    assert_eq!(selected.transcription, ai_acceleration::Mode::None);
    assert_eq!(selected.face_scoring, ai_acceleration::Mode::None);
    let unsupported = serde_json::json!({ "aiAcceleration": { "transcription": "cuda" } });
    assert!(ai_acceleration::selection_from_config(Some(&unsupported)).is_err());
}

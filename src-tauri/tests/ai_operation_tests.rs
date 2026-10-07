// Deterministic integration tests over the production face and transcription
// operations. Only native inference is substituted; cache and receipt truth
// remain owned by the same code used by the application.

use std::cell::{Cell, RefCell};

use onecopy_lib::derived_state;
use onecopy_lib::derived_work::{
    complete_transcription_attempt_with_inference, TranscriptionAttempt,
    TranscriptionAttemptOutcome,
};
use onecopy_lib::face::{complete_face_scoring_attempt, Face, FaceScoringAttemptOutcome, FoundFace};
use onecopy_lib::transcription::{Segment, Transcript};
use onecopy_lib::{index_store, preview};
use rusqlite::{params, OptionalExtension};

fn insert(conn: &rusqlite::Connection, hash: &str, kind: &str, path: &str) {
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind, derived_at_utc, derived_version)
         VALUES (?1, 1, ?2, 'ready', 4)",
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

fn spoken(text: &str) -> Transcript {
    Transcript {
        language: Some("en".to_string()),
        segments: vec![Segment { start_ms: 0, end_ms: 1_500, text: text.to_string() }],
    }
}

/// The stored transcript's plain text, if one was kept.
fn stored_text(conn: &rusqlite::Connection, hash: &str) -> Option<String> {
    conn.query_row("SELECT text FROM transcripts WHERE content_hash = ?1", [hash], |row| row.get(0))
        .optional()
        .unwrap()
}

enum TranscriptResult {
    Text(Transcript),
    Failure(String),
    Cancelled,
}

struct TranscriptScenario {
    progress: Vec<i32>,
    result: TranscriptResult,
}

impl TranscriptScenario {
    fn run(self, on_progress: &mut dyn FnMut(i32)) -> Result<Transcript, String> {
        for value in self.progress {
            on_progress(value);
        }
        match self.result {
            TranscriptResult::Text(text) => Ok(text),
            TranscriptResult::Failure(message) => Err(message),
            TranscriptResult::Cancelled => Err(onecopy_lib::scanner::CANCELLED.to_string()),
        }
    }
}

fn transcript_attempt<'a>(
    conn: &'a rusqlite::Connection,
    cache: &'a preview::CachePaths,
    data_root: &'a std::path::Path,
    hash: &'a str,
    path: &'a str,
    replace_existing: bool,
) -> TranscriptionAttempt<'a> {
    TranscriptionAttempt {
        conn,
        cache,
        data_root,
        temp_dir: data_root.join("temp"),
        source_hash: hash,
        source_path: path,
        replace_existing,
        acceleration: onecopy_lib::ai_acceleration::Mode::None,
        cancel_when: None,
    }
}

#[test]
fn audio_and_video_use_one_transcript_publication_and_restart_contract() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    insert(&conn, "audio", "audio", "voice.flac");
    insert(&conn, "video", "video", "clip.mp4");
    let cache = preview::CachePaths::new(root.path().join("cache"));

    for (hash, path) in [("audio", "voice.flac"), ("video", "clip.mp4")] {
        let starts = Cell::new(0);
        let progress = RefCell::new(Vec::new());
        let outcome = complete_transcription_attempt_with_inference(
            transcript_attempt(&conn, &cache, root.path(), hash, path, false),
            |_| {},
            |_| starts.set(starts.get() + 1),
            |_, value| progress.borrow_mut().push(value),
            |on_progress| {
                TranscriptScenario {
                    progress: vec![0, 25, 100],
                    result: TranscriptResult::Text(spoken("canonical speech")),
                }
                .run(on_progress)
            },
        )
        .unwrap();
        assert_eq!(starts.get(), 1);
        assert_eq!(*progress.borrow(), [0, 25, 100]);
        assert_eq!(
            outcome,
            TranscriptionAttemptOutcome::Completed {
                hash: hash.to_string(),
                text: "[0:00] canonical speech\n".to_string(),
                issues_changed: false,
            }
        );
    }
    drop(conn);

    let reopened = index_store::open(&db).unwrap();
    for hash in ["audio", "video"] {
        let result = derived_state::transcript_result(&reopened, hash).unwrap();
        assert_eq!(result.status, derived_state::READY);
        assert_eq!(result.text.as_deref(), Some("[0:00] canonical speech\n"));
        let (model, language, segments): (String, Option<String>, String) = reopened
            .query_row(
                "SELECT model, language, segments FROM transcripts WHERE content_hash = ?1",
                [hash],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(model, "whisper-large-v3-turbo");
        assert_eq!(language.as_deref(), Some("en"));
        assert_eq!(segments, r#"[{"startMs":0,"endMs":1500,"text":"canonical speech"}]"#);
    }
}

#[test]
fn cancellation_wins_over_a_late_success_before_transcript_publication() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let cache = preview::CachePaths::new(root.path().join("cache"));
    insert(&conn, "late", "audio", "late.flac");
    let mut attempt = transcript_attempt(
        &conn,
        &cache,
        root.path(),
        "late",
        "late.flac",
        false,
    );
    attempt.cancel_when = Some(Box::new(|| true));

    let outcome = complete_transcription_attempt_with_inference(
        attempt,
        |_| {},
        |_| {},
        |_, _| {},
        |_| Ok(spoken("late result")),
    )
    .unwrap();

    assert_eq!(
        outcome,
        TranscriptionAttemptOutcome::Cancelled {
            hash: "late".to_string(),
        }
    );
    assert_eq!(stored_text(&conn, "late"), None);
    assert_eq!(
        derived_state::transcript_result(&conn, "late")
            .unwrap()
            .status,
        "pending"
    );
}

#[test]
fn transcript_empty_failure_cancellation_retry_and_replacement_share_one_owner() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let cache = preview::CachePaths::new(root.path().join("cache"));
    for hash in ["empty", "failed", "cancelled", "replacement"] {
        insert(&conn, hash, "audio", &format!("{hash}.flac"));
    }

    let empty = complete_transcription_attempt_with_inference(
        transcript_attempt(&conn, &cache, root.path(), "empty", "empty.flac", false),
        |_| {},
        |_| {},
        |_, _| {},
        |_| Ok(Transcript::default()),
    )
    .unwrap();
    assert!(matches!(
        empty,
        TranscriptionAttemptOutcome::Completed { ref text, .. } if text.is_empty()
    ));
    assert_eq!(stored_text(&conn, "empty").as_deref(), Some(""));

    let failed = complete_transcription_attempt_with_inference(
        transcript_attempt(&conn, &cache, root.path(), "failed", "failed.flac", false),
        |_| {},
        |_| {},
        |_, _| {},
        |_| Err("deterministic failure".to_string()),
    )
    .unwrap();
    assert!(matches!(
        failed,
        TranscriptionAttemptOutcome::Failed { ref message, .. }
            if message == "deterministic failure"
    ));
    assert_eq!(stored_text(&conn, "failed"), None);

    let retried = complete_transcription_attempt_with_inference(
        transcript_attempt(&conn, &cache, root.path(), "failed", "failed.flac", false),
        |_| {},
        |_| {},
        |_, _| {},
        |_| Ok(spoken("recovered")),
    )
    .unwrap();
    assert!(matches!(
        retried,
        TranscriptionAttemptOutcome::Completed { .. }
    ));

    let cancelled = complete_transcription_attempt_with_inference(
        transcript_attempt(
            &conn,
            &cache,
            root.path(),
            "cancelled",
            "cancelled.flac",
            false,
        ),
        |_| {},
        |_| {},
        |_, _| {},
        |on_progress| {
            TranscriptScenario {
                progress: vec![0, 25],
                result: TranscriptResult::Cancelled,
            }
            .run(on_progress)
        },
    )
    .unwrap();
    assert_eq!(
        cancelled,
        TranscriptionAttemptOutcome::Cancelled {
            hash: "cancelled".to_string(),
        }
    );
    assert_eq!(stored_text(&conn, "cancelled"), None);

    complete_transcription_attempt_with_inference(
        transcript_attempt(
            &conn,
            &cache,
            root.path(),
            "replacement",
            "replacement.flac",
            false,
        ),
        |_| {},
        |_| {},
        |_, _| {},
        |_| Ok(spoken("retained result")),
    )
    .unwrap();
    let replacement = complete_transcription_attempt_with_inference(
        transcript_attempt(
            &conn,
            &cache,
            root.path(),
            "replacement",
            "replacement.flac",
            true,
        ),
        |_| {},
        |_| {},
        |_, _| {},
        |on_progress| {
            TranscriptScenario {
                progress: vec![0, 50],
                result: TranscriptResult::Failure("replacement failed".to_string()),
            }
            .run(on_progress)
        },
    )
    .unwrap();
    assert!(matches!(
        replacement,
        TranscriptionAttemptOutcome::Failed { ref message, .. }
            if message == "replacement failed"
    ));
    assert_eq!(stored_text(&conn, "replacement").as_deref(), Some("retained result"));
    assert_eq!(
        derived_state::transcript_result(&conn, "replacement")
            .unwrap()
            .status,
        derived_state::READY
    );
}

fn write_preview(cache: &preview::CachePaths, hash: &str) {
    let target = cache.preview(hash);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    image::DynamicImage::new_rgb8(2, 2).save(target).unwrap();
}

#[test]
fn face_success_empty_failure_and_cancellation_use_the_production_operation() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let cache = preview::CachePaths::new(root.path().join("cache"));
    for hash in ["smile", "none", "failed", "cancelled"] {
        insert(&conn, hash, "image", &format!("{hash}.jpg"));
        write_preview(&cache, hash);
    }
    let changed = RefCell::new(Vec::new());
    let smile = complete_face_scoring_attempt(
        &conn,
        &cache,
        "smile",
        "smile.jpg",
        &|| false,
        |hash| changed.borrow_mut().push(hash.to_string()),
        |_| {
            let face = Face { confidence: 0.75, x1: 0.1, y1: 0.1, x2: 0.4, y2: 0.5 };
            let mut expression = [0.0; 8];
            expression[4] = 1.0;
            Ok(vec![
                FoundFace { face, expression: Some(expression) },
                FoundFace { face: Face { confidence: 0.8, ..face }, expression: None },
            ])
        },
    )
    .unwrap();
    assert_eq!(
        smile,
        FaceScoringAttemptOutcome::Completed {
            faces: 2,
            issues_changed: false
        }
    );
    let none = complete_face_scoring_attempt(
        &conn,
        &cache,
        "none",
        "none.jpg",
        &|| false,
        |hash| changed.borrow_mut().push(hash.to_string()),
        |_| Ok(Vec::new()),
    )
    .unwrap();
    assert_eq!(
        none,
        FaceScoringAttemptOutcome::Completed {
            faces: 0,
            issues_changed: false
        }
    );
    let failed = complete_face_scoring_attempt(
        &conn,
        &cache,
        "failed",
        "failed.jpg",
        &|| false,
        |hash| changed.borrow_mut().push(hash.to_string()),
        |_| Err("detector failed".to_string()),
    )
    .unwrap();
    assert!(matches!(
        failed,
        FaceScoringAttemptOutcome::Failed { ref message, issues_changed: true } if message == "detector failed"
    ));
    let cancelled = complete_face_scoring_attempt(
        &conn,
        &cache,
        "cancelled",
        "cancelled.jpg",
        &|| true,
        |_| panic!("cancelled face attempt must not publish a change"),
        |_| Err(onecopy_lib::scanner::CANCELLED.to_string()),
    )
    .unwrap();
    assert_eq!(cancelled, FaceScoringAttemptOutcome::Cancelled);

    let score = |hash: &str| -> Option<f64> {
        conn.query_row("SELECT score FROM face_scores WHERE content_hash = ?1", [hash], |row| row.get(0))
            .optional()
            .unwrap()
    };
    // The smiling face outweighs the more confident one it could not read.
    assert_eq!(score("smile"), Some(0.75));
    assert_eq!(score("none"), Some(0.0));
    assert_eq!(score("failed"), None);
    let stored: (i64, Option<f64>, Option<f64>) = conn
        .query_row(
            "SELECT (SELECT face_count FROM face_checks WHERE content_hash = 'smile'),
                    (SELECT happiness FROM faces WHERE content_hash = 'smile' AND confidence = 0.75),
                    (SELECT happiness FROM faces WHERE content_hash = 'smile' AND confidence > 0.75)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(stored, (2, Some(1.0), None));
    let failures: Vec<(String, String, String)> = conn
        .prepare("SELECT content_hash, event, message FROM records.analysis_events WHERE class = 'faces'")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(failures, [("failed".to_string(), "failed".to_string(), "detector failed".to_string())]);
    assert_eq!(*changed.borrow(), ["smile", "none", "failed"]);
}

#[test]
fn face_inference_panic_records_one_failure_and_the_next_item_completes() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let cache = preview::CachePaths::new(root.path().join("cache"));
    for hash in ["panic", "healthy"] {
        insert(&conn, hash, "image", &format!("{hash}.jpg"));
        write_preview(&cache, hash);
        let outcome = complete_face_scoring_attempt(&conn, &cache, hash, &format!("{hash}.jpg"), &|| false, |_| {}, |_| {
            if hash == "panic" { panic!("synthetic inference panic"); }
            Ok(Vec::new())
        }).unwrap();
        assert_eq!(matches!(outcome, FaceScoringAttemptOutcome::Failed { .. }), hash == "panic");
    }
    assert_eq!(conn.query_row("SELECT count(*) FROM active_issues WHERE kind = 'face-score-error'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

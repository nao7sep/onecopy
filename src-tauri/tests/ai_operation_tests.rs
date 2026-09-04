// Deterministic integration tests over the production face and transcription
// operations. Only native inference is substituted; cache and receipt truth
// remain owned by the same code used by the application.

use std::cell::{Cell, RefCell};

use onecopy_lib::derived_state;
use onecopy_lib::derived_work::{
    complete_transcription_attempt_with_inference, TranscriptionAttempt,
    TranscriptionAttemptOutcome,
};
use onecopy_lib::face::{complete_face_scoring_attempt, FaceScoringAttemptOutcome};
use onecopy_lib::{index_store, preview};
use rusqlite::{params, OptionalExtension};

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

enum TranscriptResult {
    Text(String),
    Failure(String),
    Cancelled,
}

struct TranscriptScenario {
    progress: Vec<i32>,
    result: TranscriptResult,
}

impl TranscriptScenario {
    fn run(self, on_progress: &mut dyn FnMut(i32)) -> Result<String, String> {
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
                    result: TranscriptResult::Text("[0:00] canonical speech\n".to_string()),
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
            }
        );
    }
    drop(conn);

    let reopened = index_store::open(&db).unwrap();
    for hash in ["audio", "video"] {
        let result = derived_state::transcript_result(&reopened, &cache, hash).unwrap();
        assert_eq!(result.status, derived_state::READY);
        assert_eq!(result.text.as_deref(), Some("[0:00] canonical speech\n"));
    }
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
        |_| Ok(String::new()),
    )
    .unwrap();
    assert!(matches!(
        empty,
        TranscriptionAttemptOutcome::Completed { ref text, .. } if text.is_empty()
    ));
    assert_eq!(
        std::fs::read_to_string(cache.transcript("empty")).unwrap(),
        ""
    );

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
    assert!(!cache.transcript("failed").exists());

    let retried = complete_transcription_attempt_with_inference(
        transcript_attempt(&conn, &cache, root.path(), "failed", "failed.flac", false),
        |_| {},
        |_| {},
        |_, _| {},
        |_| Ok("[0:00] recovered\n".to_string()),
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
    assert!(!cache.transcript("cancelled").exists());

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
        |_| Ok("[0:00] retained result\n".to_string()),
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
    assert_eq!(
        std::fs::read_to_string(cache.transcript("replacement")).unwrap(),
        "[0:00] retained result\n"
    );
    assert_eq!(
        derived_state::transcript_result(&conn, &cache, "replacement")
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
        |_| Ok(0.75),
    )
    .unwrap();
    assert_eq!(smile, FaceScoringAttemptOutcome::Completed { score: 0.75 });
    let none = complete_face_scoring_attempt(
        &conn,
        &cache,
        "none",
        "none.jpg",
        &|| false,
        |hash| changed.borrow_mut().push(hash.to_string()),
        |_| Ok(0.0),
    )
    .unwrap();
    assert_eq!(none, FaceScoringAttemptOutcome::Completed { score: 0.0 });
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
        FaceScoringAttemptOutcome::Failed { ref message } if message == "detector failed"
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

    let scores: (f64, f64) = conn
        .query_row(
            "SELECT a.face_score, b.face_score FROM contents a, contents b
             WHERE a.hash = 'smile' AND b.hash = 'none'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(scores, (0.75, 0.0));
    let failed_state: String = conn
        .query_row(
            "SELECT face_state FROM analysis_receipts WHERE content_hash = 'failed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failed_state, derived_state::FAILED);
    let cancelled_state = conn
        .query_row(
            "SELECT face_state FROM analysis_receipts WHERE content_hash = 'cancelled'",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .unwrap()
        .flatten();
    assert_eq!(cancelled_state, None);
    assert_eq!(*changed.borrow(), ["smile", "none", "failed"]);
}

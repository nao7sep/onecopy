// Tests exercising the crate's public API from outside shipped source.

use std::cell::Cell;

use onecopy_lib::background_work::snapshot;
use onecopy_lib::derived_state;
use onecopy_lib::derived_work::{
    complete_transcription_attempt, ensure_exact_identity, priority_candidates,
    priority_candidates_for_class, settings_from_config, FaceAssets, SectionPriority,
    TranscriptionAttempt, TranscriptionAttemptOutcome,
};
use onecopy_lib::index_store;
use rusqlite::params;

#[test]
fn transcription_promotes_a_provisional_identity_before_owning_a_result() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-transcript-identity-")
        .tempdir()
        .unwrap();
    let media = dir.path().join("voice.m4a");
    std::fs::write(&media, b"the exact audio bytes").unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('p1', ?1, 'audio')",
        [std::fs::metadata(&media).unwrap().len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, missing)
         VALUES (?1, ?2, 'voice.m4a', 'audio', 'p1', 0)",
        params![media.to_string_lossy(), dir.path().to_string_lossy()],
    )
    .unwrap();

    let cache = onecopy_lib::preview::CachePaths::new(dir.path().join("cache"));
    let exact = ensure_exact_identity(&conn, &cache, "p1", &media).unwrap();

    assert_eq!(exact, onecopy_lib::hashing::full_hash(&media).unwrap());
    let path_hash: String = conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'voice.m4a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(path_hash, exact);
    let provisional_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contents WHERE hash = 'p1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(provisional_rows, 0);
}

#[test]
fn transcription_attempt_owns_cached_publication_and_dependency_classification() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-transcription-attempt-")
        .tempdir()
        .unwrap();
    let cached_media = dir.path().join("cached.m4a");
    let uncached_media = dir.path().join("uncached.m4a");
    std::fs::write(&cached_media, b"audio fixture").unwrap();
    std::fs::write(&uncached_media, b"audio fixture").unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('cached', 13, 'audio');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('uncached', 13, 'audio');",
    )
    .unwrap();
    for (hash, media) in [
        ("cached", cached_media.as_path()),
        ("uncached", uncached_media.as_path()),
    ] {
        conn.execute(
            "INSERT INTO paths
               (abs_path, dir_path, file_name, kind, content_hash, missing)
             VALUES (?1, ?2, ?3, 'audio', ?4, 0)",
            params![
                media.to_string_lossy(),
                dir.path().to_string_lossy(),
                format!("{hash}.m4a"),
                hash,
            ],
        )
        .unwrap();
    }

    let cache = onecopy_lib::preview::CachePaths::new(dir.path().join("cache"));
    let transcript = cache.transcript("cached");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "already complete").unwrap();
    let starts = Cell::new(0);

    let completed = complete_transcription_attempt(
        TranscriptionAttempt {
            conn: &conn,
            cache: &cache,
            data_root: dir.path(),
            temp_dir: dir.path().join("temp"),
            source_hash: "cached",
            source_path: cached_media.to_str().unwrap(),
            replace_existing: false,
            acceleration: onecopy_lib::ai_acceleration::Mode::None,
            observer: &onecopy_lib::ai_measurement::NOOP,
            cancel_when: None,
        },
        |_| {},
        |_| {
            starts.set(starts.get() + 1);
        },
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        completed,
        TranscriptionAttemptOutcome::Completed {
            hash: "cached".to_string(),
            text: "already complete".to_string(),
        }
    );
    let published = derived_state::transcript_result(&conn, &cache, "cached").unwrap();
    assert_eq!(published.status, derived_state::READY);
    assert_eq!(published.text.as_deref(), Some("already complete"));
    assert_eq!(starts.get(), 0);

    let cancelled = complete_transcription_attempt(
        TranscriptionAttempt {
            conn: &conn,
            cache: &cache,
            data_root: dir.path(),
            temp_dir: dir.path().join("temp"),
            source_hash: "uncached",
            source_path: uncached_media.to_str().unwrap(),
            replace_existing: false,
            acceleration: onecopy_lib::ai_acceleration::Mode::None,
            observer: &onecopy_lib::ai_measurement::NOOP,
            cancel_when: Some(Box::new(|| true)),
        },
        |_| {},
        |_| {
            starts.set(starts.get() + 1);
        },
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        cancelled,
        TranscriptionAttemptOutcome::Cancelled {
            hash: "uncached".to_string(),
        }
    );
    assert_eq!(starts.get(), 0);

    let unavailable = complete_transcription_attempt(
        TranscriptionAttempt {
            conn: &conn,
            cache: &cache,
            data_root: dir.path(),
            temp_dir: dir.path().join("temp"),
            source_hash: "uncached",
            source_path: uncached_media.to_str().unwrap(),
            replace_existing: false,
            acceleration: onecopy_lib::ai_acceleration::Mode::None,
            observer: &onecopy_lib::ai_measurement::NOOP,
            cancel_when: None,
        },
        |_| {},
        |_| {
            starts.set(starts.get() + 1);
        },
        |_, _| {},
    )
    .unwrap();
    assert!(matches!(
        unavailable,
        TranscriptionAttemptOutcome::Unavailable { ref hash, .. } if hash == "uncached"
    ));
    let pending = derived_state::transcript_result(&conn, &cache, "uncached").unwrap();
    assert_eq!(pending.status, "pending");
    assert_eq!(pending.message, None);
    assert_eq!(starts.get(), 0);
}

#[test]
fn selected_visible_and_section_backlog_keep_their_priority_without_duplicates() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derived-priority-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('selected', 1, 'image');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('visible', 1, 'image');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('backlog', 1, 'image');
         INSERT INTO contents (hash, byte_size, kind, derived_at_utc)
           VALUES ('ready', 1, 'image', 'done');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('video', 1, 'video');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/selected.jpg', '/', 'selected.jpg', 'image', 'selected', 120, 'metadata');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/visible.jpg', '/', 'visible.jpg', 'image', 'visible', 130, 'metadata');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/backlog.jpg', '/', 'backlog.jpg', 'image', 'backlog', 140, 'metadata');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/ready.jpg', '/', 'ready.jpg', 'image', 'ready', 150, 'metadata');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/video.mov', '/', 'video.mov', 'video', 'video', 160, 'metadata');",
    )
    .unwrap();
    conn.execute(
        "UPDATE contents SET derived_version = ?1 WHERE hash = 'ready'",
        [onecopy_lib::preview::DERIVE_VERSION],
    )
    .unwrap();

    let settings = settings_from_config(None, dir.path()).unwrap();
    let section = SectionPriority {
        kind: "image".to_string(),
        start_ms: Some(100),
        end_ms: Some(200),
    };
    let visible = vec![
        "visible".to_string(),
        "selected".to_string(),
        "ready".to_string(),
    ];
    let candidates =
        priority_candidates(&conn, &settings, Some("selected"), &visible, Some(&section)).unwrap();

    assert_eq!(candidates, ["selected", "visible", "backlog"]);
}

#[test]
fn every_item_class_uses_selected_visible_then_open_section_priority() {
    let dir = tempfile::tempdir().unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents
           (hash, byte_size, kind, derived_at_utc, derived_version, duration_ms)
         VALUES
           ('image-selected', 1, 'image', 'ready', 3, NULL),
           ('image-visible', 1, 'image', 'ready', 3, NULL),
           ('image-section', 1, 'image', 'ready', 3, NULL),
           ('video-selected', 1, 'video', 'ready', 3, 60000),
           ('video-visible', 1, 'video', 'ready', 3, 60000),
           ('video-section', 1, 'video', 'ready', 3, 60000),
           ('audio-selected', 1, 'audio', NULL, 0, NULL),
           ('audio-visible', 1, 'audio', NULL, 0, NULL),
           ('audio-section', 1, 'audio', NULL, 0, NULL);
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash,
            resolved_utc_ms, resolved_source)
         SELECT '/' || hash, '/', hash, kind, hash,
                CASE
                  WHEN hash LIKE '%selected' THEN 110
                  WHEN hash LIKE '%visible' THEN 120
                  ELSE 130
                END,
                'metadata'
         FROM contents;",
    )
    .unwrap();

    let mut settings = settings_from_config(None, dir.path()).unwrap();
    settings.ffmpeg = Some(dir.path().join("ffmpeg"));
    settings.face_enabled = true;
    settings.face_models = Some(FaceAssets {
        runtime: None,
        detector: dir.path().join("detector"),
        emotion: dir.path().join("emotion"),
    });
    settings.transcription_model = Some(dir.path().join("whisper"));
    let image_section = SectionPriority {
        kind: "image".to_string(),
        start_ms: Some(100),
        end_ms: Some(200),
    };
    let video_section = SectionPriority {
        kind: "video".to_string(),
        start_ms: Some(100),
        end_ms: Some(200),
    };

    let faces = priority_candidates_for_class(
        &conn,
        &settings,
        "faces",
        Some("image-selected"),
        &["image-visible".to_string()],
        Some(&image_section),
    )
    .unwrap();
    assert_eq!(faces, ["image-selected", "image-visible", "image-section"]);

    for class in ["snapshots", "video-transcripts"] {
        let candidates = priority_candidates_for_class(
            &conn,
            &settings,
            class,
            Some("video-selected"),
            &["video-visible".to_string()],
            Some(&video_section),
        )
        .unwrap();
        assert_eq!(
            candidates,
            ["video-selected", "video-visible", "video-section"],
            "{class}"
        );
    }

    let audio_section = SectionPriority {
        kind: "other".to_string(),
        start_ms: Some(100),
        end_ms: Some(200),
    };
    let audio = priority_candidates_for_class(
        &conn,
        &settings,
        "audio-transcripts",
        Some("audio-selected"),
        &["audio-visible".to_string()],
        Some(&audio_section),
    )
    .unwrap();
    assert_eq!(audio, ["audio-selected", "audio-visible", "audio-section"]);
}

#[test]
fn snapshot_projects_output_debt_without_inventing_jobs() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derived-snapshot-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('image', 1, 'image');
         INSERT INTO contents (hash, byte_size, kind, derived_at_utc)
           VALUES ('broken', 1, 'image', 'failed');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/image.jpg', '/', 'image.jpg', 'image', 'image', 120, 'metadata');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/broken.jpg', '/', 'broken.jpg', 'image', 'broken', 130, 'metadata');",
    )
    .unwrap();

    let value = serde_json::to_value(
        snapshot(
            dir.path(),
            onecopy_lib::derived_runtime::snapshot(
                onecopy_lib::derived_runtime::RuntimeConditions {
                    busy: false,
                },
            )
            .unwrap(),
            onecopy_lib::derived_work::work_capabilities(dir.path()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let rows = value["classes"].as_array().unwrap();
    let previews = rows.iter().find(|row| row["id"] == "previews").unwrap();
    let faces = rows.iter().find(|row| row["id"] == "faces").unwrap();

    assert_eq!(previews["queued"], 1);
    assert_eq!(previews["failed"], 1);
    assert_eq!(previews["state"], "queued");
    assert_eq!(faces["state"], "unavailable");
}

#[test]
fn snapshot_keeps_video_preview_debt_visible_without_ffmpeg() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derived-blocked-video-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('video', 1, 'video');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/video.mov', '/', 'video.mov', 'video', 'video', 120, 'metadata');",
    )
    .unwrap();

    let value = serde_json::to_value(
        snapshot(
            dir.path(),
            onecopy_lib::derived_runtime::snapshot(
                onecopy_lib::derived_runtime::RuntimeConditions {
                    busy: false,
                },
            )
            .unwrap(),
            onecopy_lib::derived_work::work_capabilities(dir.path()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let previews = value["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "previews")
        .unwrap();

    assert_eq!(previews["queued"], 1);
    assert_eq!(previews["state"], "unavailable");
    assert_eq!(previews["reason"], "Waiting for ffmpeg");
}

#[test]
fn one_snapshot_preserves_every_fixed_class_debt_semantic() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derived-complete-snapshot-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents
           (hash, byte_size, kind, duration_ms, strip_frames, derived_at_utc,
            derived_version)
         VALUES
           ('image-preview', 1, 'image', NULL, NULL, NULL, 0),
           ('image-face', 1, 'image', NULL, NULL, 'ready', 3),
           ('image-face-failed', 1, 'image', NULL, NULL, 'ready', 3),
           ('video-preview', 1, 'video', 60000, NULL, NULL, 0),
           ('video-snapshot', 1, 'video', 60000, NULL, 'ready', 3),
           ('video-snapshot-failed', 1, 'video', 60000, -1, 'ready', 3),
           ('video-transcript-failed', 1, 'video', 60000, 1, 'ready', 3),
           ('audio-transcript', 1, 'audio', NULL, NULL, NULL, 0),
           ('audio-transcript-failed', 1, 'audio', NULL, NULL, NULL, 0);
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash,
            resolved_utc_ms, resolved_source)
         SELECT '/' || hash, '/', hash, kind, hash, 120, 'metadata'
         FROM contents;
         INSERT INTO analysis_receipts (content_hash, face_state)
           VALUES ('image-face-failed', 'failed');
         INSERT INTO analysis_receipts (content_hash, transcript_state)
           VALUES ('video-transcript-failed', 'failed');
         INSERT INTO analysis_receipts (content_hash, transcript_state)
           VALUES ('audio-transcript-failed', 'failed');",
    )
    .unwrap();

    let value = serde_json::to_value(
        snapshot(
            dir.path(),
            onecopy_lib::derived_runtime::snapshot(
                onecopy_lib::derived_runtime::RuntimeConditions {
                    busy: false,
                },
            )
            .unwrap(),
            derived_state::WorkCapabilities {
                ffmpeg: true,
                video_snapshots_enabled: true,
                similarity_enabled: true,
                face_enabled: true,
                face_models: true,
                transcription_model: true,
                video_transcription_enabled: true,
                audio_transcription_enabled: true,
            },
        )
        .unwrap(),
    )
    .unwrap();
    let rows = value["classes"].as_array().unwrap();
    let debt = |id: &str| {
        let row = rows.iter().find(|row| row["id"] == id).unwrap();
        (
            row["queued"].as_u64().unwrap(),
            row["failed"].as_u64().unwrap(),
        )
    };

    assert_eq!(debt("previews"), (2, 0));
    assert_eq!(debt("snapshots"), (1, 1));
    assert_eq!(debt("similarity"), (1, 0));
    assert_eq!(debt("faces"), (1, 1));
    assert_eq!(debt("video-transcripts"), (3, 1));
    assert_eq!(debt("audio-transcripts"), (1, 1));
}

#[test]
fn fixed_class_candidate_reads_seek_to_the_next_ordered_page() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derived-pages-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    for index in 0..100 {
        for kind in ["image", "video"] {
            let hash = format!("{kind}-{index:03}");
            conn.execute(
                "INSERT INTO contents
                   (hash, byte_size, kind, duration_ms, derived_at_utc, derived_version)
                 VALUES (?1, 1, ?2, ?3, 'ready', ?4)",
                params![
                    hash,
                    kind,
                    (kind == "video").then_some(30_000i64),
                    derived_state::DERIVE_VERSION
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO paths
                   (abs_path, dir_path, file_name, kind, content_hash,
                    resolved_utc_ms, resolved_source)
                 VALUES (?1, '/', ?2, ?3, ?4, ?5, 'metadata')",
                params![
                    format!("/{kind}-{index:03}"),
                    format!("{kind}-{index:03}"),
                    kind,
                    hash,
                    index as i64
                ],
            )
            .unwrap();
        }
    }

    let strips = derived_state::strip_candidates(&conn, None, 7).unwrap();
    let faces = derived_state::face_candidates(&conn, None, 9).unwrap();
    let transcripts = derived_state::transcript_candidates(&conn, "video", None, 11).unwrap();

    assert_eq!(strips.len(), 7);
    assert_eq!(faces.len(), 9);
    assert_eq!(transcripts.len(), 11);
    assert_eq!(strips[0].0, "video-000");
    assert_eq!(faces[0].0, "image-000");
    assert_eq!(transcripts[0].0, "video-000");

    let next_strips =
        derived_state::strip_candidates(&conn, Some(&strips.last().unwrap().0), 7).unwrap();
    let next_faces =
        derived_state::face_candidates(&conn, Some(&faces.last().unwrap().0), 9).unwrap();
    let next_transcripts = derived_state::transcript_candidates(
        &conn,
        "video",
        Some(&transcripts.last().unwrap().0),
        11,
    )
    .unwrap();
    assert_eq!(next_strips[0].0, "video-007");
    assert_eq!(next_faces[0].0, "image-009");
    assert_eq!(next_transcripts[0].0, "video-011");
}

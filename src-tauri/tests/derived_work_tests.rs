// Tests exercising the crate's public API from outside shipped source.

use onecopy_lib::background_work::snapshot;
use onecopy_lib::derived_state;
use onecopy_lib::derived_work::{priority_candidates, settings_from_config, SectionPriority};
use onecopy_lib::index_store;
use rusqlite::params;

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

    let settings = settings_from_config(None, dir.path());
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
            onecopy_lib::derived_work::runtime_snapshot().unwrap(),
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
    assert_eq!(faces["state"], "disabled");
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
            onecopy_lib::derived_work::runtime_snapshot().unwrap(),
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
                   (hash, byte_size, kind, duration_ms, derived_at_utc)
                 VALUES (?1, 1, ?2, ?3, 'ready')",
                params![hash, kind, (kind == "video").then_some(30_000i64)],
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
    let transcripts = derived_state::transcript_candidates(&conn, None, 11).unwrap();

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
    let next_transcripts =
        derived_state::transcript_candidates(&conn, Some(&transcripts.last().unwrap().0), 11)
            .unwrap();
    assert_eq!(next_strips[0].0, "video-007");
    assert_eq!(next_faces[0].0, "image-009");
    assert_eq!(next_transcripts[0].0, "video-011");
}

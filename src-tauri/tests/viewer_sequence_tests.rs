use chrono_tz::Tz;
use onecopy_lib::derived_state;
use onecopy_lib::index_store;
use onecopy_lib::queries;
use onecopy_lib::storage;
use onecopy_lib::viewer_sequence;
use rusqlite::{params, Connection};

fn projection() -> queries::ItemProjectionContext {
    queries::ItemProjectionContext {
        capabilities: derived_state::WorkCapabilities {
            ffmpeg: true,
            video_snapshots_enabled: true,
            similarity_enabled: true,
            face_enabled: false,
            face_models: false,
            transcription_model: false,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
        },
        similarity_dirty: false,
    }
}

fn seed_image(conn: &Connection, index: u64) {
    let hash = format!("h{index}");
    let name = format!("{index}.jpg");
    conn.execute(
        "INSERT INTO contents (hash, kind, byte_size, width, height) \
         VALUES (?1, 'image', 100, 640, 480)",
        [&hash],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths \
           (abs_path, dir_path, file_name, stem, ext, kind, size, content_hash, \
            resolved_utc_ms, resolved_source, missing) \
         VALUES (?1, '/root', ?2, ?3, 'jpg', 'image', 100, ?4, 1767225600000, \
                 'metadata', 0)",
        params![format!("/root/{name}"), name, index.to_string(), hash],
    )
    .unwrap();
}

#[test]
fn disk_backed_sequence_freezes_order_and_skips_disappeared_members() {
    let root = tempfile::tempdir().unwrap();
    let index_path = root.path().join(storage::INDEX_DB_FILE_NAME);
    let conn = index_store::open(&index_path).unwrap();
    for index in 1..=5 {
        seed_image(&conn, index);
    }
    let selected = vec![queries::PositionedSectionIdentity {
        hash: Some("h3".to_string()),
        path_id: 3,
        index: 2,
    }];
    let anchor = queries::SectionIdentity {
        hash: Some("h3".to_string()),
        path_id: 3,
    };
    let snapshot = viewer_sequence::start(
        root.path(),
        &conn,
        "image",
        "2026-01",
        Tz::UTC,
        queries::SectionSort {
            order: queries::SectionSortOrder::Name,
            desc: false,
        },
        selected,
        &anchor,
        projection(),
    )
    .unwrap();
    assert_eq!(snapshot.length, 5);
    assert_eq!(snapshot.index, 2);
    assert_eq!(snapshot.item.hash.as_deref(), Some("h3"));

    conn.execute("DELETE FROM paths WHERE content_hash = 'h4'", [])
        .unwrap();
    let reconciled = viewer_sequence::reconcile(&snapshot.token, &index_path, &conn, projection())
        .unwrap()
        .unwrap();
    assert_eq!(reconciled.length, 4);

    let next = viewer_sequence::move_current(
        &snapshot.token,
        viewer_sequence::Move::Next,
        &conn,
        projection(),
    )
    .unwrap();
    assert_eq!(next.item.hash.as_deref(), Some("h5"));
    assert_eq!(next.index, 3);
    viewer_sequence::close(Some(&snapshot.token)).unwrap();
    assert!(!root
        .path()
        .join(onecopy_lib::binaries_manager::TEMP_DIR_NAME)
        .join("viewer-sequence.sqlite3")
        .exists());
}

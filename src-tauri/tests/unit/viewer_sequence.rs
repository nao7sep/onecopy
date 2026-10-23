use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn navigation_reuses_position_until_membership_or_failed_projection_changes_it() {
    let root = tempfile::tempdir().unwrap();
    let index_path = root.path().join(crate::storage::INDEX_DB_FILE_NAME);
    let conn = crate::index_store::open(&index_path).unwrap();
    let projection = ItemProjectionContext {
        capabilities: crate::derived_state::WorkCapabilities {
            ffmpeg: false, video_snapshots_enabled: false, similarity_enabled: false,
            face_enabled: false, face_models: false, transcription_model: false,
            transcription_acceleration: true, face_acceleration: true,
            video_transcription_enabled: false, audio_transcription_enabled: false,
        },
    };
    let mut selected = Vec::new();
    for index in 1..=5i64 {
        let hash = format!("h{index}");
        conn.execute("INSERT INTO contents (hash, byte_size, kind, width, height)
                      VALUES (?1, 1, 'image', 2, 2)", [&hash]).unwrap();
        conn.execute("INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
                      VALUES (?1, '/pics', ?2, 'image', ?3)",
            params![format!("/pics/{index}.png"), format!("{index}.png"), hash]).unwrap();
        selected.push(PositionedSectionIdentity { hash: Some(hash), path_id: index, index: (index - 1) as u64 });
    }
    let anchor = SectionIdentity { hash: Some("h3".into()), path_id: 3 };
    let initial = start(root.path(), &conn, queries::SectionKind::Image, "undated", Tz::UTC,
        SectionSort { order: queries::SectionSortOrder::Name, desc: false },
        selected, &anchor, projection).unwrap();
    assert_eq!((initial.index, initial.length), (2, 5));
    let counts = Arc::new(AtomicUsize::new(0));
    let observed = counts.clone();
    ACTIVE.lock().unwrap().as_ref().unwrap().conn.authorizer(Some(move |context: AuthContext<'_>| {
        if matches!(context.action, AuthAction::Function { function_name } if function_name.eq_ignore_ascii_case("count")) {
            observed.fetch_add(1, Ordering::SeqCst);
        }
        Authorization::Allow
    })).unwrap();
    let step = |movement, index, length, hash: &str| {
        let snapshot = move_current(&initial.token, movement, &conn, projection).unwrap();
        assert_eq!((snapshot.index, snapshot.length), (index, length));
        assert_eq!(snapshot.item.hash.as_deref(), Some(hash));
    };
    step(Move::Next, 3, 5, "h4");
    step(Move::Last, 4, 5, "h5");
    step(Move::Next, 4, 5, "h5");
    step(Move::Previous, 3, 5, "h4");
    step(Move::First, 0, 5, "h1");
    step(Move::Next, 1, 5, "h2");
    assert_eq!(counts.load(Ordering::SeqCst), 0, "ordinary and end navigation must not recount members");

    conn.execute("DELETE FROM paths WHERE content_hash = 'h2'", []).unwrap();
    let reconciled = reconcile(&initial.token, &index_path, &conn, projection).unwrap().unwrap();
    assert_eq!((reconciled.index, reconciled.length), (1, 4));
    assert_eq!(reconciled.item.hash.as_deref(), Some("h3"));
    assert!(counts.load(Ordering::SeqCst) > 0, "membership changes must refresh position");
    counts.store(0, Ordering::SeqCst);
    step(Move::Previous, 0, 4, "h1");
    step(Move::Next, 1, 4, "h3");
    reconcile(&initial.token, &index_path, &conn, projection).unwrap().unwrap();
    assert_eq!(counts.load(Ordering::SeqCst), 0, "unchanged reconciliation must retain position");

    // Lazy retirement during navigation has the same invalidation owner.
    conn.execute("DELETE FROM paths WHERE content_hash = 'h4'", []).unwrap();
    step(Move::Next, 2, 3, "h5");
    assert!(counts.load(Ordering::SeqCst) > 0);
    step(Move::First, 0, 3, "h1");
    conn.execute("UPDATE contents SET width = 'invalid' WHERE hash = 'h3'", []).unwrap();
    assert!(move_current(&initial.token, Move::Next, &conn, projection).is_err());
    conn.execute("UPDATE contents SET width = 2 WHERE hash = 'h3'", []).unwrap();
    step(Move::Next, 1, 3, "h3");
    step(Move::Last, 2, 3, "h5");
    close(Some(&initial.token)).unwrap();
}

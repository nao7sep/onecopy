use super::{expression_crop_bounds, Face};

#[test]
fn expression_crop_omits_degenerate_and_outside_boxes() {
    let face = Face { confidence: 1.0, x1: 0.5, y1: 0.5, x2: 0.5, y2: 0.5 };
    assert_eq!(expression_crop_bounds(100, 100, &face), None);
    let outside = Face { x1: 1.1, x2: 1.2, y1: 0.1, y2: 0.3, ..face };
    assert_eq!(expression_crop_bounds(100, 100, &outside), None);
    let usable = Face { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0, ..face };
    assert_eq!(expression_crop_bounds(100, 80, &usable), Some((0, 0, 100, 80)));
}

#[test]
fn completion_keeps_captured_model_identity_after_later_evidence_changes() {
    use super::{complete_face_scoring_attempt_with_model, FaceScoringAttemptOutcome, FoundFace};
    use crate::{ai_dependencies, derived_state, index_store, preview};
    use rusqlite::params;

    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let cache = preview::CachePaths::new(root.path().join("cache"));
    let captured = ai_dependencies::ModelIdentity {
        model: "loaded-detector+loaded-expression".to_string(),
        version: "loaded-detector-digest+loaded-expression-digest".to_string(),
    };
    // The injected adapter represents a session that has already captured
    // its identity. Later observations must not replace that session's facts.
    let mut later_evidence = captured.clone();
    for hash in ["success", "failure"] {
        conn.execute(
            "INSERT INTO contents (hash, byte_size, kind) VALUES (?1, 1, 'image')",
            [hash],
        ).unwrap();
        let target = cache.preview(hash);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        image::DynamicImage::new_rgb8(2, 2).save(target).unwrap();
        let outcome = complete_face_scoring_attempt_with_model(
            &conn, &cache, hash, &format!("{hash}.jpg"), &|| false, |_| {},
            |_| {
                later_evidence.version = format!("replacement-{hash}");
                if hash == "failure" {
                    Err("expression inference failed".to_string())
                } else {
                    Ok(vec![FoundFace {
                        face: Face { confidence: 1.0, x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 },
                        expression: Some([0.125; 8]),
                    }])
                }
            },
            &captured,
        ).unwrap();
        assert_ne!(later_evidence, captured);
        assert_eq!(matches!(outcome, FaceScoringAttemptOutcome::Failed { .. }), hash == "failure");
        let stored: (String, String) = if hash == "success" {
            let face: (String, String) = conn.query_row(
                "SELECT model, model_version FROM faces WHERE content_hash = ?1", [hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
            assert_eq!(face, (captured.model.clone(), captured.version.clone()));
            conn.query_row(
                "SELECT model, model_version FROM face_checks WHERE content_hash = ?1", [hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap()
        } else {
            conn.query_row(
                "SELECT model, model_version FROM records.analysis_events
                 WHERE content_hash = ?1 AND event = ?2",
                params![hash, derived_state::FAILED],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap()
        };
        assert_eq!(stored, (captured.model.clone(), captured.version.clone()));
    }
}

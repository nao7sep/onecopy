// Face scoring's model-free contracts (Phase 28 test doctrine): the ordering
// the score buys, the fallback a model-less install keeps, and the pure
// geometry the detector's post-processing runs on. The LIVE test at the
// bottom downloads the real pinned artifacts and proves the runtime binding
// against a face-free image — the sanity floor that needs no real faces in
// the corpus.

use onecopy_lib::face::{self, Face};
use onecopy_lib::{index_store, queries};
use rusqlite::{params, Connection};

fn db() -> Connection {
    let dir = tempfile::Builder::new().prefix("onecopy-face-db-").tempdir().unwrap();
    // Leak the tempdir handle so the SQLite file outlives this helper; the OS
    // temp cleaner owns it, the same lifetime every DB-backed test here uses.
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    std::mem::forget(dir);
    conn
}

fn seed_image(conn: &Connection, hash: &str, name: &str) {
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind, derived_at_utc, sharpness) \
         VALUES (?1, 1, 'image', '2026-01-01T00:00:00.000Z', 5.0)",
        params![hash],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
         VALUES (?1, '/pics', ?2, 'image', ?3)",
        params![format!("/pics/{name}"), name, hash],
    )
    .unwrap();
}

fn group(conn: &Connection, hashes: &[&str]) {
    conn.execute("INSERT INTO similar_groups (id, created_at_utc) VALUES (1, 'x')", [])
        .unwrap();
    for hash in hashes {
        conn.execute(
            "INSERT INTO similar_group_members (group_id, content_hash) VALUES (1, ?1)",
            params![hash],
        )
        .unwrap();
    }
}

#[test]
fn face_score_orders_ahead_of_sharpness_within_the_group() {
    let conn = db();
    for (hash, name) in [("smile", "a.jpg"), ("blur", "b.jpg"), ("scenery", "c.jpg")] {
        seed_image(&conn, hash, name);
    }
    // The sharpest member has NO face; the softest has the best face. The
    // design's promise: the smiling face wins the slot, sharpness only
    // breaks face ties.
    conn.execute("UPDATE contents SET sharpness = 9.0, face_score = 0.0 WHERE hash = 'scenery'", []).unwrap();
    conn.execute("UPDATE contents SET sharpness = 2.0, face_score = 0.91 WHERE hash = 'smile'", []).unwrap();
    conn.execute("UPDATE contents SET sharpness = 4.0, face_score = 0.55 WHERE hash = 'blur'", []).unwrap();
    group(&conn, &["smile", "blur", "scenery"]);

    let members = queries::similar_group_of(&conn, "scenery").unwrap();
    assert_eq!(
        members.iter().map(|m| m.hash.as_str()).collect::<Vec<_>>(),
        vec!["smile", "blur", "scenery"],
        "face-bearing members first, best face first"
    );
    // The comparison surface renders from this row — the score must ride it.
    assert_eq!(members[0].face_score, Some(0.91));
}

#[test]
fn null_scores_fall_back_to_sharpness_exactly_as_before() {
    let conn = db();
    for (hash, name) in [("sharp", "a.jpg"), ("soft", "b.jpg")] {
        seed_image(&conn, hash, name);
    }
    // NULL face_score throughout — a model-less install, or a faceless group
    // scored as 0.0: COALESCE makes both order purely by sharpness.
    conn.execute("UPDATE contents SET sharpness = 9.0 WHERE hash = 'sharp'", []).unwrap();
    conn.execute("UPDATE contents SET sharpness = 1.0, face_score = 0.0 WHERE hash = 'soft'", []).unwrap();
    group(&conn, &["sharp", "soft"]);

    let members = queries::similar_group_of(&conn, "sharp").unwrap();
    assert_eq!(
        members.iter().map(|m| m.hash.as_str()).collect::<Vec<_>>(),
        vec!["sharp", "soft"],
        "no face anywhere -> sharpest first, the pre-model ordering"
    );
}

#[test]
fn comparison_payload_serializes_the_score_camel_cased() {
    // The broadcast to the comparison windows is serde JSON; the frontend
    // mirror reads `faceScore`.
    let conn = db();
    seed_image(&conn, "one", "a.jpg");
    conn.execute("UPDATE contents SET face_score = 0.5 WHERE hash = 'one'", []).unwrap();
    group(&conn, &["one"]);
    let members = queries::similar_group_of(&conn, "one").unwrap();
    let json = serde_json::to_value(&members[0]).unwrap();
    assert_eq!(json["faceScore"], serde_json::json!(0.5));
    assert!(json.get("face_score").is_none(), "camelCase only");
}

#[test]
fn model_less_pass_is_a_silent_no_op_leaving_scores_null() {
    let dir = tempfile::Builder::new().prefix("onecopy-face-").tempdir().unwrap();
    let conn = db();
    seed_image(&conn, "img", "a.jpg");
    let cache = onecopy_lib::preview::CachePaths::new(dir.path().join("cache"));

    let stats = face::face_scores_pending(&conn, &cache, None, |_, _| {}).unwrap();
    assert_eq!((stats.scored, stats.failed), (0, 0));
    let score: Option<f64> = conn
        .query_row("SELECT face_score FROM contents WHERE hash = 'img'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(score, None, "no models -> untouched, ordering identical to today");
}

#[test]
fn nms_keeps_the_best_of_an_overlapping_pile_and_all_distinct_faces() {
    let at = |x: f32, confidence: f32| Face { confidence, x1: x, y1: 0.1, x2: x + 0.2, y2: 0.3 };
    let kept = face::non_max_suppression(vec![
        at(0.10, 0.80),
        at(0.11, 0.95), // same face, higher confidence — the survivor
        at(0.60, 0.75), // a genuinely different face
    ]);
    assert_eq!(kept.len(), 2);
    assert_eq!(kept[0].confidence, 0.95, "best-first");
    assert_eq!(kept[1].confidence, 0.75);
}

#[test]
fn iou_and_softmax_hold_their_edges() {
    let unit = Face { confidence: 1.0, x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 };
    assert_eq!(face::iou(&unit, &unit), 1.0);
    let apart = Face { confidence: 1.0, x1: 2.0, y1: 2.0, x2: 3.0, y2: 3.0 };
    assert_eq!(face::iou(&unit, &apart), 0.0);
    let degenerate = Face { confidence: 1.0, x1: 0.5, y1: 0.5, x2: 0.5, y2: 0.5 };
    assert_eq!(face::iou(&degenerate, &degenerate), 0.0, "zero-area boxes never overlap");

    let probabilities = face::softmax(&[1.0, 3.0, 1.0]);
    assert!((probabilities.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    assert!(probabilities[1] > probabilities[0]);
    assert!(face::softmax(&[]).is_empty());
    // Large logits must not overflow to NaN — the stability the max-shift buys.
    assert!(face::softmax(&[1000.0, 999.0]).iter().all(|p| p.is_finite()));
}

// Run with `cargo test live_face_models -- --ignored --nocapture`.
#[test]
#[ignore]
fn live_face_models_find_nothing_in_a_face_free_image() {
    use onecopy_lib::binaries_manager::spec_of;
    use sha2::Digest;

    let dir = tempfile::Builder::new().prefix("onecopy-face-live-").tempdir().unwrap();
    let fetch = |id: &str, name: &str| {
        let pin = spec_of(id).unwrap().pinned.as_ref().unwrap();
        let path = dir.path().join(name);
        let mut response = ureq::get(pin.url).call().expect("download");
        let mut file = std::fs::File::create(&path).unwrap();
        std::io::copy(&mut response.body_mut().as_reader(), &mut file).unwrap();
        let mut hasher = sha2::Sha256::new();
        hasher.update(std::fs::read(&path).unwrap());
        assert_eq!(hex::encode(hasher.finalize()), pin.sha256, "{id} integrity");
        path
    };
    let detector = fetch("ultraface-rfb320", "det.onnx");
    let emotion = fetch("emotion-ferplus", "emo.onnx");

    // A gradient has structure but no face; a broken binding (wrong output
    // wiring, wrong preprocessing) shows up as phantom faces or an error.
    let gradient = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(640, 480, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
    }));
    let mut scorer = face::FaceScorer::load(&detector, &emotion).unwrap();
    let score = scorer.score(&gradient).unwrap();
    eprintln!("face-free gradient scored {score}");
    assert_eq!(score, 0.0, "no face may be found where none exists");
}

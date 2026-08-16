// Embedding pairing's model-free contracts (Phase 28 doctrine): the cosine
// arithmetic, the leader-bounded clustering, the group merge, and the whole
// rebuild driven by hand-inserted embedding BLOBs — everything except the
// encoder itself, which the ignored live test proves with the real pinned
// model.

use onecopy_lib::embedding::*;
use onecopy_lib::index_store;
use onecopy_lib::similarity::{merge_clusters, rebuild_groups, SimilarityConfig};
use rusqlite::{params, Connection};

#[test]
fn cosine_is_the_dot_product_and_degenerates_safely() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    assert_eq!(cosine(&a, &a), 1.0);
    assert_eq!(cosine(&a, &b), 0.0);
    // Mismatched or empty inputs read as no-similarity, never a panic — a
    // degenerate row must not poison a rebuild.
    assert_eq!(cosine(&a, &[1.0, 0.0, 0.0]), 0.0);
    assert_eq!(cosine(&[], &[]), 0.0);
}

#[test]
fn the_blob_codec_round_trips_and_rejects_junk() {
    let vector = vec![0.25f32, -1.5, 3.75];
    assert_eq!(from_blob(&to_blob(&vector)).unwrap(), vector);
    assert!(from_blob(&[1, 2, 3]).is_none(), "non-multiple-of-4 is junk");
    assert!(from_blob(&[]).is_none());
}

#[test]
fn clusters_are_leader_bounded_and_skip_the_embeddingless() {
    // a and b within threshold of a (the leader); c far from a founds its own
    // cluster even though it is within threshold of b — leader bounding is
    // exactly what stops cosine chaining. d has no embedding and never joins.
    let e = |x: f32, y: f32| {
        let n = (x * x + y * y).sqrt();
        Some(vec![x / n, y / n])
    };
    let embeddings = vec![
        e(1.0, 0.0),        // a — leader
        e(0.95, 0.31225),   // b: cos(a,b) ≈ 0.95
        e(0.80, 0.60),      // c: cos(a,c) = 0.80 < 0.9, founds cluster 2
        None,               // d
        e(0.999, 0.0447),   // e: cos(a,e) ≈ 0.999 → joins a
    ];
    let clusters = embedding_clusters(&embeddings, 0.9);
    assert_eq!(clusters, vec![vec![0, 1, 4]], "c stays alone (dropped: len < 2)");
}

#[test]
fn merging_unions_groups_through_a_cluster_and_leaves_the_rest_alone() {
    // Groups {0,1} and {2,3}; a cluster links 1 and 2 (and loner 4): all five
    // become one group. Group {5,6} is untouched.
    let groups = vec![vec![0, 1], vec![2, 3], vec![5, 6]];
    let clusters = vec![vec![1, 2, 4]];
    let merged = merge_clusters(&groups, &clusters);
    assert_eq!(merged, vec![vec![0, 1, 2, 3, 4], vec![5, 6]]);
}

/// Seeds one image row with a camera, a phash, and an optional embedding.
fn seed(conn: &Connection, hash: &str, camera: &str, phash: i64, embedding: Option<&[f32]>) {
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind, phash, sharpness, camera_make, embedding) \
         VALUES (?1, 100, 'image', ?2, 1.0, ?3, ?4)",
        params![hash, phash, camera, embedding.map(to_blob)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms, \
         content_hash, resolved_utc_ms, resolved_source, date_only, missing, companion_of) \
         VALUES (?1, '/b', ?2, ?3, 'jpg', 'image', 100, 0, ?4, 1700000000000, 'metadata', 0, 0, NULL)",
        params![
            format!("/b/{hash}.jpg"),
            format!("{hash}.jpg"),
            hash,
            hash
        ],
    )
    .unwrap();
}

fn db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-embed-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    (dir, conn)
}

#[test]
fn cross_camera_photos_group_by_embedding_alone() {
    // The whole point: two devices' renderings of one scene — phashes far
    // apart (dHash cannot pair them), embeddings near-identical. With the
    // threshold on they form ONE group across the camera partition; with the
    // feature off (None) they never meet — today's behaviour is the fallback.
    let (_d, conn) = db();
    let scene = [0.6f32, 0.8];
    seed(&conn, "phone", "Apple|", 0b0000, Some(&scene));
    seed(&conn, "camera", "Sony|", 0b111111111111, Some(&[0.61, 0.7924]));

    let on = SimilarityConfig {
        max_gap_seconds: 90,
        phash_max_distance: 4,
        embedding_min_cosine: Some(0.95),
    };
    let stats = rebuild_groups(&conn, &on).unwrap();
    assert_eq!(stats.groups, 1, "embedding must pair across cameras");
    assert_eq!(stats.grouped_items, 2);

    let off = SimilarityConfig {
        max_gap_seconds: 90,
        phash_max_distance: 4,
        embedding_min_cosine: None,
    };
    let stats = rebuild_groups(&conn, &off).unwrap();
    assert_eq!(stats.groups, 0, "disabled means dHash-only");
}

#[test]
fn embedding_merges_do_not_disturb_pure_visual_groups() {
    // Two visually-identical same-camera shots with NO embeddings still group
    // exactly as before the feature existed, threshold on or off.
    let (_d, conn) = db();
    seed(&conn, "v1", "Ricoh|", 0b0001, None);
    seed(&conn, "v2", "Ricoh|", 0b0011, None);
    let on = SimilarityConfig {
        max_gap_seconds: 90,
        phash_max_distance: 4,
        embedding_min_cosine: Some(0.9),
    };
    let stats = rebuild_groups(&conn, &on).unwrap();
    assert_eq!(stats.groups, 1);
    assert_eq!(stats.grouped_items, 2);
}

// LIVE: downloads the real pinned model (~335 MB), verifies its sha256, and
// asserts the semantic the feature stands on: two structurally similar
// generated images embed closer than dissimilar ones, and the same image at
// two scales closest of all. Run:
//   cargo test --test embedding_tests -- --ignored --nocapture
#[test]
#[ignore]
fn live_model_orders_similarity_sanely() {
    use image::DynamicImage;
    use onecopy_lib::binaries_manager::{spec_of};

    let pin = spec_of("siglip2-large-vision").unwrap().pinned.as_ref().unwrap();
    let dir = tempfile::Builder::new()
        .prefix("onecopy-embed-live-")
        .tempdir()
        .unwrap();
    let model = dir.path().join("model.onnx");
    let mut response = ureq::get(pin.url).call().expect("download");
    let mut file = std::fs::File::create(&model).unwrap();
    std::io::copy(&mut response.body_mut().as_reader(), &mut file).unwrap();
    {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(std::fs::read(&model).unwrap());
        assert_eq!(hex::encode(hasher.finalize()), pin.sha256, "model integrity");
    }

    // Scene A: a bright disc on a dark field. Scene A': the same disc, other
    // scale. Scene B: stripes — structurally different.
    let disc = |size: u32| {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(size, size, |x, y| {
            let cx = size as f32 / 2.0;
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cx).powi(2)).sqrt();
            if d < size as f32 / 4.0 {
                image::Rgb([240, 220, 90])
            } else {
                image::Rgb([25, 30, 60])
            }
        }))
    };
    let stripes = DynamicImage::ImageRgb8(image::RgbImage::from_fn(512, 512, |x, _| {
        if (x / 32) % 2 == 0 {
            image::Rgb([230, 40, 40])
        } else {
            image::Rgb([40, 230, 40])
        }
    }));

    let mut embedder = Embedder::load(&model).unwrap();
    let a = embedder.embed(&disc(512)).unwrap();
    let a_scaled = embedder.embed(&disc(256)).unwrap();
    let b = embedder.embed(&stripes).unwrap();

    let same = cosine(&a, &a_scaled);
    let different = cosine(&a, &b);
    eprintln!("dims {} | same-scene cosine {same:.4}, different-scene {different:.4}", a.len());
    assert!(same > different, "the same scene must embed closer");
    assert!(same > 0.9, "scale must barely move the embedding: {same}");

    // SPEED IS AN ACCEPTANCE CRITERION, not a curiosity: this pass runs over
    // every image in the library, and a per-image cost that turns the
    // developer's worst month (~30k photos) into DAYS rather than hours is a
    // rejection however good the embedding is. Measured after a warm-up so
    // the first-run graph setup is not counted as steady state.
    let _ = embedder.embed(&disc(384)).unwrap();
    let started = std::time::Instant::now();
    const RUNS: u32 = 5;
    for _ in 0..RUNS {
        let _ = embedder.embed(&disc(512)).unwrap();
    }
    let per_image = started.elapsed() / RUNS;
    let month = per_image * 30_000;
    eprintln!(
        "per image {:?} → a 30k-photo month costs {:.1} h",
        per_image,
        month.as_secs_f64() / 3600.0
    );
    assert!(
        month.as_secs_f64() / 3600.0 < 24.0,
        "a 30k month must stay within hours, not days: {:?}/image",
        per_image
    );
}

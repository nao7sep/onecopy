// Embedding pairing's model-free contracts (Phase 28 doctrine): the cosine
// arithmetic, the leader-bounded clustering, the group merge, and the whole
// rebuild driven by hand-inserted embedding BLOBs — everything except the
// encoder itself, which the ignored live test proves with the real pinned
// model.

use onecopy_lib::embedding::*;
use onecopy_lib::index_store;
use onecopy_lib::similarity::{rebuild_groups, SimilarityConfig};
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
fn a_visual_group_never_bridges_two_unrelated_subjects_into_one_family() {
    // THE REGRESSION THAT MATTERED, in the exact shape that bit: a visual
    // group acts as a BRIDGE. Y is two shots of one subject that pair by
    // dhash. X resembles Y's first member; Z resembles Y's second; X and Z
    // resemble nothing of each other.
    //
    // Stage B once unioned every visual group that shared an embedding
    // cluster, transitively — so {X,y1} and {y2,Z} both touched Y and fused
    // X with Z. On the developer's real library that chained 207 unrelated
    // app icons (butterflies, microphones, cassettes) into ONE group whose
    // median pair sat at cosine 0.72 and dhash distance 28. Bounding the
    // merge to a leader is what keeps a false pair costing one wrong
    // neighbour instead of the whole bucket.
    let (_d, conn) = db();
    let at = |deg: f32| {
        let r: f32 = (deg as f32).to_radians();
        [r.cos(), r.sin()]
    };
    // Y: one subject, two shots — near-identical phashes, same camera, so
    // dhash pairs them. Their EMBEDDINGS sit far apart (the subject moved),
    // which is what makes the group a bridge rather than a blob.
    seed(&conn, "y1", "Sony|", 0b0001, Some(&at(0.0)));
    seed(&conn, "y2", "Sony|", 0b0011, Some(&at(90.0)));
    // X pairs with y1 (cos 20° ≈ 0.94) and Z with y2 (cos 20° ≈ 0.94), while
    // X vs Z is cos 90° = 0 — as unrelated as two images get. The angles are
    // spread so that NO single leader can absorb both ends: whichever item
    // the clustering starts from, two separate clusters form and each touches
    // one half of Y. That is precisely the bridge the old union walked.
    seed(&conn, "x", "Apple|", 0b1111_0000_0000_0000, Some(&at(20.0)));
    seed(&conn, "z", "Nikon|", 0b0000_1111_1111_0000, Some(&at(110.0)));

    let config = SimilarityConfig {
        max_gap_seconds: 90,
        diameter_multiplier: 2,
        phash_max_distance: 4,
        embedding_min_cosine: Some(0.9),
    };
    rebuild_groups(&conn, &config).unwrap();

    let mut stmt = conn
        .prepare("SELECT group_id, content_hash FROM similar_group_members")
        .unwrap();
    let mut families: std::collections::HashMap<i64, Vec<String>> = Default::default();
    for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).unwrap() {
        let (group, hash) = row.unwrap();
        families.entry(group).or_default().push(hash);
    }
    for members in families.values() {
        assert!(
            !(members.iter().any(|h| h == "x") && members.iter().any(|h| h == "z")),
            "two unrelated subjects were bridged into one family: {members:?}"
        );
    }
    // The genuine pairing still happens: Y stays whole and takes exactly the
    // one end that resembles its representative — which end depends on which
    // member represents the group, and either answer is correct.
    let with_y1 = families
        .values()
        .find(|m| m.iter().any(|h| h == "y1"))
        .expect("y1 is grouped");
    assert!(with_y1.iter().any(|h| h == "y2"), "the burst stays whole");
    assert_eq!(with_y1.len(), 3, "the burst plus ONE end: {with_y1:?}");
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
        diameter_multiplier: 2,
        phash_max_distance: 4,
        embedding_min_cosine: Some(0.95),
    };
    let stats = rebuild_groups(&conn, &on).unwrap();
    assert_eq!(stats.groups, 1, "embedding must pair across cameras");
    assert_eq!(stats.grouped_items, 2);

    let off = SimilarityConfig {
        max_gap_seconds: 90,
        diameter_multiplier: 2,
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
        diameter_multiplier: 2,
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

// LIVE, and the test this feature should have had from the start: the whole
// pipeline over REAL images, asserting the thing a user actually notices —
// that unrelated pictures do not end up in one "similar" family.
//
// Everything above is hand-built vectors. That is what let a 207-member
// hairball ship twice: the arithmetic was right in isolation, and no test
// ever put real photographs through the real model and looked at the answer.
//
// Runs against company/assets — the authorized corpus, ~626 app icons, which
// is the HARDEST case the app faces: flat art on dark rounded squares that
// crowds into one corner of both dhash and embedding space. Photographs are
// easier; if grouping is sane here it is sane there.
//
// Run with:
//   cargo test live_corpus_grouping -- --ignored --nocapture
// Set ONECOPY_TEST_EMBEDDING_MODEL to a local copy of the pinned artifact to
// skip the 1.2 GB download on a re-run.
#[test]
#[ignore]
#[serial_test::serial(backup_store)]
fn live_corpus_grouping_keeps_unrelated_icons_apart() {
    use onecopy_lib::binaries_manager::spec_of;

    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../company/assets")
        .canonicalize()
        .expect("the authorized corpus");
    // ONECOPY_TEST_KEEP_HOME leaves the index behind so the grouping can be
    // inspected afterwards — tuning a threshold means reading the pairs it
    // accepted, which a deleted temp dir makes impossible.
    let kept = std::env::var_os("ONECOPY_TEST_KEEP_HOME").map(std::path::PathBuf::from);
    let home = tempfile::Builder::new()
        .prefix("onecopy-corpus-live-")
        .tempdir()
        .unwrap();
    let home_path = match &kept {
        Some(path) => {
            std::fs::create_dir_all(path).unwrap();
            path.clone()
        }
        None => home.path().to_path_buf(),
    };

    let pin = spec_of("siglip2-large-vision").unwrap().pinned.as_ref().unwrap();
    let model = match std::env::var_os("ONECOPY_TEST_EMBEDDING_MODEL") {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let path = home_path.join("model.onnx");
            let mut response = ureq::get(pin.url).call().expect("download");
            let mut file = std::fs::File::create(&path).unwrap();
            std::io::copy(&mut response.body_mut().as_reader(), &mut file).unwrap();
            path
        }
    };
    {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(std::fs::read(&model).unwrap());
        assert_eq!(hex::encode(hasher.finalize()), pin.sha256, "model integrity");
    }

    let conn = index_store::open(&home_path.join("index.sqlite3")).unwrap();
    let config = serde_json::json!({
        "sourceDirs": [corpus.to_string_lossy()],
        "defaultTimezone": "UTC",
    });
    let mut settings =
        onecopy_lib::scanner::settings_from_config(Some(&config), &home_path, 1_800_000_000_000);
    settings.embedding_model = Some(model);
    onecopy_lib::scanner::run_full_scan(&conn, &settings, &|phase, detail| {
        if phase == "group" || phase == "embed" {
            eprintln!("  {phase}: {detail}");
        }
    })
    .expect("the scan completes");

    let images: i64 = conn
        .query_row("SELECT COUNT(*) FROM contents WHERE kind = 'image'", [], |r| r.get(0))
        .unwrap();
    let embedded: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contents WHERE embedding IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(images > 100, "the corpus should yield real work: {images}");
    assert_eq!(embedded, images, "every image must be embedded");

    let mut stmt = conn
        .prepare("SELECT group_id, COUNT(*) FROM similar_group_members GROUP BY group_id ORDER BY 2 DESC")
        .unwrap();
    let sizes: Vec<(i64, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let largest = sizes.first().map(|(_, n)| *n).unwrap_or(0);
    let grouped: i64 = sizes.iter().map(|(_, n)| n).sum();
    eprintln!(
        "{images} images → {} groups, largest {largest}, grouped {grouped}",
        sizes.len()
    );

    // THE HAIRBALL GUARD. Not a quality bar — a floor. This corpus holds
    // genuine families (one icon rendered at several sizes), so groups of a
    // dozen are correct; a group holding a tenth of everything is the chain
    // this test exists to catch. The failure it replaces was 207 of 548.
    assert!(
        largest * 10 < images,
        "one group swallowed {largest} of {images} images — that is a chain, \
         not a family. Sizes: {:?}",
        &sizes[..sizes.len().min(8)]
    );
}

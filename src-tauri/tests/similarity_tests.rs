// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::index_store;
use onecopy_lib::similarity::*;
use rusqlite::{params, Connection};

fn seeded() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-sim-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    (dir, conn)
}

fn config() -> SimilarityConfig {
    SimilarityConfig {
        phash_max_distance_burst: 10,
        max_gap_seconds: 90,
        diameter_multiplier: 2,
        phash_max_distance: 4,
    }
}

fn insert_image_with_camera(
    conn: &Connection,
    hash: &str,
    camera: Option<&str>,
    time_ms: Option<i64>,
    phash: i64,
    sharpness: f64,
) {
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind, phash, camera_make, camera_model, sharpness) \
         VALUES (?1, 1, 'image', ?2, ?3, ?4, ?5)",
        params![hash, phash, camera, camera.map(|_| "M"), sharpness],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source) \
         VALUES (?1, '/a', ?2, 'image', ?3, ?4, 'metadata')",
        params![format!("/a/{hash}.jpg"), format!("{hash}.jpg"), hash, time_ms],
    )
    .unwrap();
}

fn insert_image(
    conn: &Connection,
    hash: &str,
    camera: &str,
    time_ms: i64,
    phash: i64,
    sharpness: f64,
) {
    insert_image_with_camera(conn, hash, Some(camera), Some(time_ms), phash, sharpness);
}

#[test]
fn spare_shots_within_the_gap_group_together() {
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    insert_image(&conn, "s1", "Ricoh", t, 0b0000, 10.0);
    insert_image(&conn, "s2", "Ricoh", t + 20_000, 0b0011, 30.0);
    insert_image(&conn, "s3", "Ricoh", t + 45_000, 0b0111, 20.0);
    // Visually identical to s3 but far outside the gap: the burst
    // refinement separates it, and alone it forms no group.
    insert_image(&conn, "lone", "Ricoh", t + 600_000, 0b0111, 5.0);

    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(stats.groups, 1);
    assert_eq!(stats.grouped_items, 3);

    let group_id: i64 = conn
        .query_row("SELECT id FROM similar_groups", [], |r| r.get(0))
        .unwrap();
    // Best-first by sharpness: s2 (30) leads.
    assert_eq!(
        group_members(&conn, group_id).unwrap(),
        vec!["s2", "s3", "s1"]
    );
}

#[test]
fn cancellation_keeps_the_previous_complete_similarity_cohort() {
    let (_dir, conn) = seeded();
    let t = 1_700_000_000_000i64;
    insert_image(&conn, "a", "Ricoh", t, 0, 1.0);
    insert_image(&conn, "b", "Ricoh", t + 1_000, 1, 1.0);
    rebuild_groups(&conn, &config()).unwrap();
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM similar_group_members", [], |row| {
            row.get(0)
        })
        .unwrap();

    conn.execute(
        "UPDATE contents SET camera_model = 'changed' WHERE hash = 'a'",
        [],
    )
    .unwrap();
    let error = rebuild_next_dirty_bucket_cancellable(&conn, &config(), &|| true).unwrap_err();
    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM similar_group_members", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert_eq!(error, onecopy_lib::scanner::CANCELLED);
    assert_eq!(after, before);
    assert_eq!(dirty_bucket_count(&conn).unwrap(), 1);
}

#[test]
fn different_cameras_never_chain() {
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    insert_image(&conn, "a1", "Ricoh", t, 0, 1.0);
    insert_image(&conn, "b1", "Sony", t + 10_000, 0, 1.0);
    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(
        stats.groups, 0,
        "cross-device grouping is the deferred phase"
    );
}

#[test]
fn a_scene_change_inside_the_gap_splits_the_chain() {
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    // Two visually close, a visual jump within the time gap, two close
    // again: two groups, never one.
    insert_image(&conn, "c1", "Ricoh", t, 0x0000_0000_0000_00FF, 1.0);
    insert_image(&conn, "c2", "Ricoh", t + 10_000, 0x0000_0000_0000_00FE, 1.0);
    insert_image(&conn, "d1", "Ricoh", t + 20_000, 0x7FFF_FFFF_FFFF_0000, 1.0);
    insert_image(&conn, "d2", "Ricoh", t + 30_000, 0x7FFF_FFFF_FFFE_0000, 1.0);

    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(stats.groups, 2);
    assert_eq!(stats.grouped_items, 4);
}

#[test]
fn interleaving_never_shatters_a_family() {
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    // The failure the neighbour chain had: an unrelated image lands
    // BETWEEN two members of a family. Bucket-and-cluster still groups
    // the family; the stranger stands apart.
    insert_image(&conn, "f1", "Ricoh", t, 0b0000, 1.0);
    insert_image(&conn, "x1", "Ricoh", t + 10_000, !0b0000, 1.0); // far
    insert_image(&conn, "f2", "Ricoh", t + 20_000, 0b0001, 1.0);

    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(stats.groups, 1);
    assert_eq!(stats.grouped_items, 2);
}

#[test]
fn camera_less_files_group_on_appearance_alone() {
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    // Screenshots/renders: no camera, times far apart within the month —
    // under the old design these could never group at all.
    insert_image_with_camera(&conn, "n1", None, Some(t), 0b0000, 1.0);
    insert_image_with_camera(&conn, "n2", None, Some(t + 86_400_000), 0b0011, 1.0);
    // And one with no resolved time at all lands in the undated bucket.
    insert_image_with_camera(&conn, "u1", None, None, 0b0000, 1.0);
    insert_image_with_camera(&conn, "u2", None, None, 0b0001, 1.0);

    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(stats.groups, 2, "one dated pair, one undated pair");
    assert_eq!(stats.grouped_items, 4);
}

#[test]
fn month_buckets_bound_the_scope() {
    let (_d, conn) = seeded();
    // Visually identical, two different months: scoped apart by design.
    let jan = 1_704_067_200_000i64; // 2024-01-01T00:00:00Z
    let mar = 1_709_251_200_000i64; // 2024-03-01T00:00:00Z
    insert_image_with_camera(&conn, "m1", None, Some(jan), 0, 1.0);
    insert_image_with_camera(&conn, "m2", None, Some(mar), 0, 1.0);
    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(stats.groups, 0);
}

#[test]
fn large_families_group_whole_with_no_cap() {
    // The cap's removal is the contract here (developer, 2026-08-16): a
    // 75-member family was previously NOT persisted at all — no group, no
    // ≈ badge — which silently hid exactly the largest spare-shot families.
    // The comparison view runs any group in 16-slot turns, so size is the
    // queue's problem, never the grouper's.
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    for i in 0..75 {
        insert_image_with_camera(&conn, &format!("o{i}"), None, Some(t + i * 1000), 0, 1.0);
    }
    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(stats.groups, 1, "one family, however large");
    assert_eq!(stats.grouped_items, 75);
    let issues: i64 = conn
        .query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))
        .unwrap();
    assert_eq!(issues, 0, "a large family is not a problem to report");
}

#[test]
fn a_chain_cannot_glue_dissimilar_photos_into_one_family() {
    // The developer's screenshot, reduced to numbers: union-find chains
    // distance-4 LINKS into a 75-member "family" whose farthest pair sat 28
    // bits apart — a ghost and a moon, "similar". A group's diameter is now
    // bounded at twice the threshold, so the chain splits where it stops
    // looking like one family.
    let hashes: Vec<i64> = vec![
        0b0000_0000_0000, // a
        0b0000_0000_1111, // b: d4 from a
        0b0000_1111_1111, // c: d4 from b, d8 from a  (still within 2d)
        0b1111_1111_1111, // d: d4 from c, d12 from a (chained past 2d)
    ];
    let clusters = cluster_by_appearance(&hashes, &vec![None; hashes.len()], 4, 4, 90, 2).unwrap();
    assert_eq!(clusters.len(), 2, "the chain must split");
    // Split at the seam, not scattered: sorted-by-hash leaders keep the near
    // pairs together.
    assert_eq!(clusters[0], vec![0, 1], "a and b stay a family");
    assert_eq!(clusters[1], vec![2, 3], "c and d stay a family");
}

#[test]
fn a_tight_family_with_spread_ends_stays_whole() {
    // A real burst: every member within the threshold of a shared middle, the
    // two ends up to 2d apart. That is one family, not a chain — splitting it
    // is the shattering the union-find design existed to prevent.
    let hashes: Vec<i64> = vec![
        0b0000_1111, // end one
        0b0000_0011, // middle (d2 from both ends)
        0b0011_0011, // end two: d4 from middle, d6 from end one (≤ 2d)
    ];
    let clusters = cluster_by_appearance(&hashes, &vec![None; hashes.len()], 4, 4, 90, 2).unwrap();
    assert_eq!(clusters.len(), 1, "within-diameter components stand whole");
    assert_eq!(clusters[0].len(), 3);
}

#[test]
fn identical_twins_survive_a_hairball_split() {
    // Same art at two sizes hashes identically (distance 0). When their
    // component is chained and must split, the twins have to land in ONE
    // cluster — hash-ordered leaders make them adjacent, so they do.
    let hashes: Vec<i64> = vec![
        0b1111_1111_1111, // far end of a chain
        0b0000_0000_0000, // twin 1
        0b0000_1111_1111, // chain middle
        0b0000_0000_0000, // twin 2
        0b0000_0000_1111, // chain link
    ];
    let clusters = cluster_by_appearance(&hashes, &vec![None; hashes.len()], 4, 4, 90, 2).unwrap();
    let twins: Vec<&Vec<usize>> = clusters
        .iter()
        .filter(|c| c.contains(&1) || c.contains(&3))
        .collect();
    assert_eq!(twins.len(), 1, "distance-0 twins must share a cluster");
}

#[test]
fn complete_bucket_rebuild_is_idempotent() {
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    insert_image(&conn, "r1", "Ricoh", t, 0, 1.0);
    insert_image(&conn, "r2", "Ricoh", t + 5_000, 1, 1.0);
    rebuild_groups(&conn, &config()).unwrap();
    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(stats.groups, 1);
    let member_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM similar_group_members", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(member_rows, 2, "no duplicate membership after a rebuild");
}

#[test]
fn rebuilding_one_dirty_month_preserves_the_other_months_publication() {
    let (_dir, conn) = seeded();
    let jan = 1_704_067_200_000i64;
    let mar = 1_709_251_200_000i64;
    insert_image(&conn, "jan-a", "Ricoh", jan, 0, 1.0);
    insert_image(&conn, "jan-b", "Ricoh", jan + 1_000, 1, 1.0);
    insert_image(&conn, "mar-a", "Ricoh", mar, 0, 1.0);
    insert_image(&conn, "mar-b", "Ricoh", mar + 1_000, 1, 1.0);
    rebuild_groups(&conn, &config()).unwrap();
    let group_id = |bucket: &str| {
        conn.query_row(
            "SELECT id FROM similar_groups WHERE bucket = ?1",
            [bucket],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
    };
    let jan_before = group_id("2024-01");
    let mar_before = group_id("2024-03");

    conn.execute("UPDATE contents SET phash = phash WHERE hash = 'jan-a'", [])
        .unwrap();
    assert_eq!(dirty_bucket_count(&conn).unwrap(), 1);
    let stats = rebuild_next_dirty_bucket_cancellable(&conn, &config(), &|| false)
        .unwrap()
        .unwrap();

    assert_eq!(stats.last_bucket.as_deref(), Some("2024-01"));
    assert_ne!(group_id("2024-01"), jan_before);
    assert_eq!(group_id("2024-03"), mar_before);
    assert_eq!(dirty_bucket_count(&conn).unwrap(), 0);
}

#[test]
fn date_change_invalidates_both_the_old_and_new_months() {
    let (_dir, conn) = seeded();
    let jan = 1_704_067_200_000i64;
    let mar = 1_709_251_200_000i64;
    insert_image(&conn, "moved", "Ricoh", jan, 0, 1.0);
    rebuild_groups(&conn, &config()).unwrap();

    conn.execute(
        "UPDATE paths SET resolved_utc_ms = ?1 WHERE content_hash = 'moved'",
        [mar],
    )
    .unwrap();
    let buckets = conn
        .prepare("SELECT bucket FROM similarity_dirty_buckets ORDER BY bucket")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(buckets, vec!["2024-01", "2024-03"]);
}

#[test]
fn pre_epoch_dates_keep_their_actual_utc_month() {
    let (_dir, conn) = seeded();
    insert_image(&conn, "old", "Ricoh", -1, 0, 1.0);

    let bucket: String = conn
        .query_row("SELECT bucket FROM similarity_dirty_buckets", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(bucket, "1969-12");
}

#[test]
fn a_new_invalidation_during_computation_cannot_publish_stale_membership() {
    use std::cell::Cell;

    let (dir, conn) = seeded();
    let t = 1_700_000_000_000i64;
    insert_image(&conn, "a", "Ricoh", t, 0, 1.0);
    insert_image(&conn, "b", "Ricoh", t + 1_000, 1, 1.0);
    rebuild_groups(&conn, &config()).unwrap();
    conn.execute(
        "UPDATE contents SET camera_model = 'first-change' WHERE hash = 'a'",
        [],
    )
    .unwrap();

    let writer = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let changed = Cell::new(false);
    let stop = || {
        if !changed.replace(true) {
            writer
                .execute(
                    "UPDATE contents SET phash = ?1 WHERE hash = 'b'",
                    [i64::MAX],
                )
                .unwrap();
        }
        false
    };
    let stats = rebuild_next_dirty_bucket_cancellable(&conn, &config(), &stop)
        .unwrap()
        .unwrap();

    assert_eq!(stats.groups, 0);
    assert_eq!(dirty_bucket_count(&conn).unwrap(), 0);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM similar_groups", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

/// Two images whose phashes pair, seeded the way the engine reads them.
fn seed_pairable(conn: &Connection, hash: &str, phash: i64) {
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind, phash, sharpness, camera_make) \
         VALUES (?1, 100, 'image', ?2, 1.0, '|')",
        params![hash, phash],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms, \
         content_hash, resolved_utc_ms, resolved_source, date_only, missing, companion_of) \
         VALUES (?1, '/b', ?2, ?3, 'jpg', 'image', 100, 0, ?4, 1700000000000, 'metadata', 0, 0, NULL)",
        params![format!("/b/{hash}.jpg"), format!("{hash}.jpg"), hash, hash],
    )
    .unwrap();
}

#[test]
fn grouping_ignores_retired_exclusion_files_and_preserves_their_bytes() {
    let (dir, conn) = seeded();
    seed_pairable(&conn, "a", 1);
    seed_pairable(&conn, "b", 3);
    let path = dir.path().join("similar-exclusions.json");
    for bytes in [
        br#"{"exclusions":[{"hashA":"a","hashB":"b","createdAtUtc":"2026-09-09T00:00:00.000Z"}]}"#
            .as_slice(),
        b"{ invalid retired data".as_slice(),
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(rebuild_groups(&conn, &config()).unwrap().grouped_items, 2);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    index_store::clear_reconstructible(&conn).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"{ invalid retired data");
}

#[test]
fn retired_grouping_version_is_invalidated_once_without_resetting_prepared_content() {
    let (dir, conn) = seeded();
    seed_pairable(&conn, "a", 1);
    seed_pairable(&conn, "b", 3);
    rebuild_groups(&conn, &config()).unwrap();
    // An older index can retain its unused column; only the current grouping
    // version participates in eligibility. Simulate a previously split pair.
    conn.execute_batch(
        "ALTER TABLE similarity_state ADD COLUMN exclusions_fingerprint TEXT NOT NULL DEFAULT '';
         UPDATE similarity_state SET config_fingerprint = '90:4:10:2', exclusions_fingerprint = 'old';
         DELETE FROM similar_group_members;
         DELETE FROM similar_groups;
         DELETE FROM similarity_dirty_buckets;
         UPDATE contents SET derived_at_utc = '2026-09-09T00:00:00.000Z', derived_version = 12;",
    ).unwrap();
    drop(conn);
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let first = rebuild_next_dirty_bucket_cancellable(&conn, &config(), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(first.grouped_items, 2);
    assert!(
        rebuild_next_dirty_bucket_cancellable(&conn, &config(), &|| false)
            .unwrap()
            .is_none()
    );
    let prepared: i64 = conn.query_row(
        "SELECT COUNT(*) FROM contents WHERE derived_at_utc = '2026-09-09T00:00:00.000Z' AND derived_version = 12",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(prepared, 2);
}

#[test]
fn a_config_change_invalidates_every_bucket_once() {
    let (_dir, conn) = seeded();
    let jan = 1_704_067_200_000i64;
    let mar = 1_709_251_200_000i64;
    insert_image(&conn, "jan", "Ricoh", jan, 0, 1.0);
    insert_image(&conn, "mar", "Ricoh", mar, 0, 1.0);
    rebuild_groups(&conn, &config()).unwrap();

    let changed = SimilarityConfig {
        phash_max_distance: 5,
        ..config()
    };
    ensure_config_current(&conn, &changed).unwrap();
    let revisions = || {
        conn.prepare("SELECT bucket, revision FROM similarity_dirty_buckets ORDER BY bucket")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let once = revisions();
    assert_eq!(once.len(), 2);

    ensure_config_current(&conn, &changed).unwrap();
    assert_eq!(
        revisions(),
        once,
        "the same config must not re-invalidate cohorts"
    );
}

// ---- Time-gated pairing (Phase 33) ----------------------------------------
// Family bursts spread wider in dhash than the icon-tuned strict line, and
// capture time is the strongest signal that frames belong together. The
// relaxed allowance applies ONLY within the burst gap; everything else —
// including everything undated — keeps the strict line, so flat art stays
// exactly as hard to group as before.

/// Two phashes exactly `bits` apart.
fn apart(bits: u32) -> (i64, i64) {
    let a: i64 = 0x0F0F_0F0F_0F0F_0F0F;
    (a, a ^ ((1i64 << bits) - 1))
}

#[test]
fn burst_close_pairs_group_at_the_relaxed_distance() {
    let (a, b) = apart(8); // past strict 3, inside burst 10
    let hashes = vec![a, b];
    let times = vec![Some(1_000_000), Some(1_005_000)]; // 5 s apart
    let clusters = cluster_by_appearance(&hashes, &times, 3, 10, 90, 2).unwrap();
    assert_eq!(clusters, vec![vec![0, 1]], "a real burst pair must group");
}

#[test]
fn the_same_distance_an_hour_apart_stays_split() {
    let (a, b) = apart(8);
    let hashes = vec![a, b];
    let times = vec![Some(1_000_000), Some(4_600_000_000)];
    let clusters = cluster_by_appearance(&hashes, &times, 3, 10, 90, 2).unwrap();
    assert_eq!(clusters.len(), 2, "far apart in time means the strict line");
}

#[test]
fn undated_pairs_never_get_the_relaxed_allowance() {
    // No capture evidence = no burst claim: the relaxation must never leak
    // to the icon corpus, whose files carry no times at all.
    let (a, b) = apart(8);
    let hashes = vec![a, b];
    let clusters = cluster_by_appearance(&hashes, &[None, None], 3, 10, 90, 2).unwrap();
    assert_eq!(clusters.len(), 2);
}

#[test]
fn a_burst_cannot_chain_into_a_far_photo() {
    // a–b are a genuine burst; c looks somewhat like b but was shot far
    // later. The pair allowance is per PAIR, so c must not ride the burst's
    // relaxed line into the group.
    let (a, b) = apart(8);
    let c = b ^ ((1i64 << 8) - 1) << 20; // 8 bits from b, 16 from a
    let hashes = vec![a, b, c];
    let times = vec![Some(1_000_000), Some(1_005_000), Some(9_000_000_000)];
    let clusters = cluster_by_appearance(&hashes, &times, 3, 10, 90, 2).unwrap();
    assert!(
        clusters.contains(&vec![0, 1]),
        "the burst survives: {clusters:?}"
    );
    assert!(
        clusters.iter().all(|cl| !cl.contains(&2) || cl.len() == 1),
        "the far photo stays out: {clusters:?}"
    );
}

#[test]
fn exact_candidates_match_exhaustive_pairing_across_strict_and_burst_rules() {
    for round in 0..8u32 {
        let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ u64::from(round);
        let mut hashes = Vec::new();
        let mut times = Vec::new();
        for index in 0..160usize {
            if index % 8 == 0 {
                state ^= state >> 30;
                state = state.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                state ^= state >> 27;
            }
            let flips = (index % 8) as u32;
            let mask = ((1u64 << flips) - 1).rotate_left((index as u32 * 7 + round) % 64);
            hashes.push((state ^ mask) as i64);
            times.push(if index % 13 == 0 {
                None
            } else {
                Some((index / 8) as i64 * 300_000 + (index % 8) as i64 * 15_000)
            });
        }

        let mut actual = cluster_by_appearance(&hashes, &times, 4, 10, 90, 64).unwrap();
        let mut expected = exhaustive_components(&hashes, &times, 4, 10, 90);
        normalize_components(&mut actual);
        normalize_components(&mut expected);
        assert_eq!(
            actual, expected,
            "candidate search lost an edge in round {round}"
        );
    }
}

fn exhaustive_components(
    hashes: &[i64],
    times: &[Option<i64>],
    strict: u32,
    burst: u32,
    gap_seconds: u32,
) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..hashes.len()).collect();
    let gap_ms = i64::from(gap_seconds) * 1000;
    for a in 0..hashes.len() {
        for b in (a + 1)..hashes.len() {
            let allowed = match (times[a], times[b]) {
                (Some(left), Some(right)) if (left - right).abs() <= gap_ms => strict.max(burst),
                _ => strict,
            };
            if (hashes[a] ^ hashes[b]).count_ones() <= allowed {
                let left = test_find(&mut parent, a);
                let right = test_find(&mut parent, b);
                parent[right] = left;
            }
        }
    }
    let mut components = std::collections::BTreeMap::<usize, Vec<usize>>::new();
    for index in 0..hashes.len() {
        let root = test_find(&mut parent, index);
        components.entry(root).or_default().push(index);
    }
    components.into_values().collect()
}

fn test_find(parent: &mut [usize], index: usize) -> usize {
    if parent[index] != index {
        parent[index] = test_find(parent, parent[index]);
    }
    parent[index]
}

fn normalize_components(components: &mut Vec<Vec<usize>>) {
    for component in components.iter_mut() {
        component.sort_unstable();
    }
    components.sort();
}

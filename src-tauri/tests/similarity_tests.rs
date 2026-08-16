// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::index_store;
use rusqlite::{params, Connection};
use onecopy_lib::similarity::*;

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
        max_gap_seconds: 90,
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
fn different_cameras_never_chain() {
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    insert_image(&conn, "a1", "Ricoh", t, 0, 1.0);
    insert_image(&conn, "b1", "Sony", t + 10_000, 0, 1.0);
    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(stats.groups, 0, "cross-device grouping is the deferred phase");
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
        insert_image_with_camera(
            &conn,
            &format!("o{i}"),
            None,
            Some(t + i * 1000),
            0,
            1.0,
        );
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
        0b0000_0000_0000,          // a
        0b0000_0000_1111,          // b: d4 from a
        0b0000_1111_1111,          // c: d4 from b, d8 from a  (still within 2d)
        0b1111_1111_1111,          // d: d4 from c, d12 from a (chained past 2d)
    ];
    let clusters = cluster_by_appearance(&hashes, 4);
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
    let clusters = cluster_by_appearance(&hashes, 4);
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
    let clusters = cluster_by_appearance(&hashes, 4);
    let twins: Vec<&Vec<usize>> =
        clusters.iter().filter(|c| c.contains(&1) || c.contains(&3)).collect();
    assert_eq!(twins.len(), 1, "distance-0 twins must share a cluster");
}

#[test]
fn rebuild_is_wholesale_and_idempotent() {
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    insert_image(&conn, "r1", "Ricoh", t, 0, 1.0);
    insert_image(&conn, "r2", "Ricoh", t + 5_000, 1, 1.0);
    rebuild_groups(&conn, &config()).unwrap();
    let stats = rebuild_groups(&conn, &config()).unwrap();
    assert_eq!(stats.groups, 1);
    let member_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM similar_group_members", [], |r| r.get(0))
        .unwrap();
    assert_eq!(member_rows, 2, "no duplicate membership after a rebuild");
}

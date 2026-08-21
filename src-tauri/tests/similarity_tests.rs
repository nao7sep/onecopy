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
    let clusters = cluster_by_appearance(&hashes, &vec![None; hashes.len()], 4, 4, 90, 2);
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
    let clusters = cluster_by_appearance(&hashes, &vec![None; hashes.len()], 4, 4, 90, 2);
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
    let clusters = cluster_by_appearance(&hashes, &vec![None; hashes.len()], 4, 4, 90, 2);
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

// ---- Unlink: the user's "not the same subject" verdicts ----

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
fn split_by_exclusions_removes_the_intruder_and_keeps_the_family_whole() {
    use std::collections::HashSet;
    let family = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // The intruder was unlinked against every member — the shape the unlink
    // command writes.
    let excluded: HashSet<(String, String)> = ["a", "b", "c"]
        .iter()
        .map(|m| {
            let (x, y) = if *m < "intruder" { (m.to_string(), "intruder".into()) } else { ("intruder".into(), m.to_string()) };
            (x, y)
        })
        .collect();
    let out = split_by_exclusions(vec![family(&["a", "b", "intruder", "c"])], &excluded);
    assert_eq!(out, vec![family(&["a", "b", "c"])], "family whole, intruder out (dropped: alone)");

    // No exclusions → untouched, same allocation path.
    let untouched = split_by_exclusions(vec![family(&["a", "b"])], &HashSet::new());
    assert_eq!(untouched, vec![family(&["a", "b"])]);
}

#[test]
fn an_unlinked_pair_never_regroups_however_similar_their_pixels_are() {
    // The persistence promise: groups are rebuilt WHOLESALE every scan, so an
    // unlink stored against the group would evaporate. Stored against the
    // pair, it must hold on every later rebuild.
    let (dir, conn) = seeded();
    seed_pairable(&conn, "keeper", 0b0001);
    seed_pairable(&conn, "bolt", 0b0011); // distance 1 — pairs on looks

    let cfg = config();
    rebuild_groups(&conn, &cfg).unwrap();
    let grouped: i64 = conn
        .query_row("SELECT COUNT(*) FROM similar_group_members", [], |r| r.get(0))
        .unwrap();
    assert_eq!(grouped, 2, "they pair before the verdict");

    let written = unlink_from_group(&conn, dir.path(), "bolt").unwrap();
    assert_eq!(written, 1, "one exclusion per other member");
    // Immediate effect, no rescan needed: membership gone, and a group of one
    // is dissolved rather than left as a phantom ≈ badge.
    let (members, groups): (i64, i64) = (
        conn.query_row("SELECT COUNT(*) FROM similar_group_members", [], |r| r.get(0)).unwrap(),
        conn.query_row("SELECT COUNT(*) FROM similar_groups", [], |r| r.get(0)).unwrap(),
    );
    assert_eq!((members, groups), (0, 0));

    // And the verdict binds every future rebuild.
    rebuild_groups_for_root(&conn, &cfg, dir.path()).unwrap();
    let regrouped: i64 = conn
        .query_row("SELECT COUNT(*) FROM similar_group_members", [], |r| r.get(0))
        .unwrap();
    assert_eq!(regrouped, 0, "the pair must never re-form");

    // Unlinking something ungrouped is a quiet no-op, not an error.
    assert_eq!(unlink_from_group(&conn, dir.path(), "keeper").unwrap(), 0);
}

#[test]
fn an_unlinked_image_still_groups_with_a_genuine_twin() {
    // The exclusion is PAIRWISE, not a ban on the image: a real duplicate of
    // the unlinked photo arriving later must still pair with it.
    let (dir, conn) = seeded();
    seed_pairable(&conn, "bolt", 0b0011);
    seed_pairable(&conn, "family", 0b0001);
    let cfg = config();
    rebuild_groups(&conn, &cfg).unwrap();
    unlink_from_group(&conn, dir.path(), "bolt").unwrap();

    seed_pairable(&conn, "bolt-copy", 0b0010); // distance 1 from bolt
    rebuild_groups_for_root(&conn, &cfg, dir.path()).unwrap();
    let bolt_group: Option<i64> = conn
        .query_row(
            "SELECT group_id FROM similar_group_members WHERE content_hash = 'bolt'",
            [],
            |r| r.get(0),
        )
        .map(Some)
        .unwrap_or(None);
    let group = bolt_group.expect("the twin pairs with the unlinked image");
    let with: Vec<String> = conn
        .prepare("SELECT content_hash FROM similar_group_members WHERE group_id = ?1 ORDER BY 1")
        .unwrap()
        .query_map([group], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(with.contains(&"bolt-copy".to_string()));
    assert!(!with.contains(&"family".to_string()), "the verdict still holds: {with:?}");
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
    let clusters = cluster_by_appearance(&hashes, &times, 3, 10, 90, 2);
    assert_eq!(clusters, vec![vec![0, 1]], "a real burst pair must group");
}

#[test]
fn the_same_distance_an_hour_apart_stays_split() {
    let (a, b) = apart(8);
    let hashes = vec![a, b];
    let times = vec![Some(1_000_000), Some(4_600_000_000)];
    let clusters = cluster_by_appearance(&hashes, &times, 3, 10, 90, 2);
    assert_eq!(clusters.len(), 2, "far apart in time means the strict line");
}

#[test]
fn undated_pairs_never_get_the_relaxed_allowance() {
    // No capture evidence = no burst claim: the relaxation must never leak
    // to the icon corpus, whose files carry no times at all.
    let (a, b) = apart(8);
    let hashes = vec![a, b];
    let clusters = cluster_by_appearance(&hashes, &[None, None], 3, 10, 90, 2);
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
    let clusters = cluster_by_appearance(&hashes, &times, 3, 10, 90, 2);
    assert!(
        clusters.contains(&vec![0, 1]),
        "the burst survives: {clusters:?}"
    );
    assert!(
        clusters.iter().all(|cl| !cl.contains(&2) || cl.len() == 1),
        "the far photo stays out: {clusters:?}"
    );
}

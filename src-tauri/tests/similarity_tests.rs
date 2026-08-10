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
        max_group_size: 32,
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
fn oversize_clusters_are_flagged_not_truncated() {
    let (_d, conn) = seeded();
    let t = 1_700_000_000_000i64;
    let tight = SimilarityConfig {
        max_gap_seconds: 90,
        phash_max_distance: 4,
        max_group_size: 3,
    };
    for i in 0..5 {
        insert_image_with_camera(
            &conn,
            &format!("o{i}"),
            None,
            Some(t + i * 1000),
            0,
            1.0,
        );
    }
    let stats = rebuild_groups(&conn, &tight).unwrap();
    assert_eq!(stats.groups, 0, "over-cap must not persist as a group");
    assert_eq!(stats.oversize_clusters, 1);
    let issues: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM issues WHERE kind = 'similar-overflow'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(issues, 1);
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

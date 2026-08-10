//! Similar-shot grouping — bucket-and-cluster: within each month bucket,
//! every pair of images within the configured dHash distance is unioned into
//! a visual cluster (union-find), so a family of near-identical shots groups
//! regardless of what was written between them and regardless of whether any
//! EXIF exists. The camera + time-gap rule is a REFINEMENT inside a cluster,
//! never a precondition: members carrying a camera split into bursts at gaps,
//! while camera-less members (screenshots, exports, renders) stand on
//! appearance alone. Cross-camera grouping stays deferred — a cluster
//! partitions by camera identity before the burst split.
//!
//! Buckets bound the pairwise work (global pairwise at millions of files is
//! not tractable, and banding cannot rescue exact recall); the cost is that a
//! family straddling a month boundary splits — accepted by design. Bucket
//! months are UTC — a scope, not a display concept; section display months
//! can differ near boundaries by the display-timezone offset.
//!
//! A cluster larger than `max_group_size` is NOT a spare-shot family; it is
//! left ungrouped and surfaced as an issue, never silently truncated —
//! offering a hundred-member "group" to a keep-one-delete-the-rest flow is
//! the destructive hazard the cap exists to prevent.
//!
//! Groups rebuild wholesale after each scan — membership is cheap to derive
//! and rebuilding sidesteps every incremental staleness bug. A group needs
//! ≥ 2 members to exist. Best-effort by design and documented as such; groups
//! order best-first by sharpness so slot 1 is the machine's guess.

use std::collections::HashMap;

use rusqlite::{params, Connection};

pub struct SimilarityConfig {
    pub max_gap_seconds: u32,
    pub phash_max_distance: u32,
    pub max_group_size: u32,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct GroupStats {
    pub groups: u64,
    pub grouped_items: u64,
    pub oversize_clusters: u64,
}

fn hamming(a: i64, b: i64) -> u32 {
    (a ^ b).count_ones()
}

/// The month bucket key: UTC `yyyy-mm`, or `undated`.
fn bucket_key(resolved_utc_ms: Option<i64>) -> String {
    match resolved_utc_ms.and_then(chrono::DateTime::from_timestamp_millis) {
        Some(dt) => dt.format("%Y-%m").to_string(),
        None => "undated".to_string(),
    }
}

/// Plain union-find over indices.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> UnionFind {
        UnionFind {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, i: usize) -> usize {
        if self.parent[i] != i {
            let root = self.find(self.parent[i]);
            self.parent[i] = root;
        }
        self.parent[i]
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

struct Candidate {
    hash: String,
    camera: String,
    time_ms: Option<i64>,
    phash: i64,
}

/// Rebuilds every similar group from the current index. Only images with a
/// perceptual hash participate; the logical item's time is the earliest among
/// its copies (the same rule the sections use), and undated images form their
/// own bucket rather than being excluded.
pub fn rebuild_groups(
    conn: &Connection,
    config: &SimilarityConfig,
) -> Result<GroupStats, String> {
    let mut stmt = conn
        .prepare(
            "SELECT c.hash, \
             COALESCE(c.camera_make, '') || '|' || COALESCE(c.camera_model, ''), \
             MIN(p.resolved_utc_ms), c.phash \
             FROM contents c JOIN paths p ON p.content_hash = c.hash \
             WHERE c.kind = 'image' AND c.phash IS NOT NULL \
               AND p.missing = 0 AND p.companion_of IS NULL \
             GROUP BY c.hash",
        )
        .map_err(|e| e.to_string())?;
    let candidates: Vec<Candidate> = stmt
        .query_map([], |r| {
            Ok(Candidate {
                hash: r.get(0)?,
                camera: r.get(1)?,
                time_ms: r.get(2)?,
                phash: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    // Partition into month buckets, then cluster within each.
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        buckets.entry(bucket_key(c.time_ms)).or_default().push(i);
    }

    let gap_ms = i64::from(config.max_gap_seconds) * 1000;
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut oversize: Vec<(String, usize)> = Vec::new();

    for (bucket, indices) in &buckets {
        // Union-find over pairs within the visual threshold. Quadratic within
        // the bucket — integer work over in-memory rows, no file reads.
        let mut uf = UnionFind::new(indices.len());
        for a in 0..indices.len() {
            for b in (a + 1)..indices.len() {
                let (ca, cb) = (&candidates[indices[a]], &candidates[indices[b]]);
                if hamming(ca.phash, cb.phash) <= config.phash_max_distance {
                    uf.union(a, b);
                }
            }
        }
        let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
        for local in 0..indices.len() {
            let root = uf.find(local);
            clusters.entry(root).or_default().push(indices[local]);
        }

        for cluster in clusters.into_values() {
            if cluster.len() < 2 {
                continue;
            }
            // Refinement: partition by camera identity (cross-camera grouping
            // is the deferred phase), then split camera-bearing partitions
            // into bursts at time gaps. The camera-less partition ("|")
            // stands whole — appearance is its only signal, by design.
            let mut by_camera: HashMap<&str, Vec<usize>> = HashMap::new();
            for idx in cluster {
                by_camera
                    .entry(candidates[idx].camera.as_str())
                    .or_default()
                    .push(idx);
            }
            for (camera, mut members) in by_camera {
                let camera_less = camera == "|";
                if camera_less {
                    if members.len() >= 2 {
                        push_or_flag(
                            members.iter().map(|&i| candidates[i].hash.clone()).collect(),
                            bucket,
                            config,
                            &mut groups,
                            &mut oversize,
                        );
                    }
                    continue;
                }
                // Burst split along time; members without a resolved time sort
                // last and never force a split among themselves.
                members.sort_by_key(|&i| candidates[i].time_ms.unwrap_or(i64::MAX));
                let mut burst: Vec<usize> = Vec::new();
                let mut last_time: Option<i64> = None;
                for idx in members {
                    let time = candidates[idx].time_ms;
                    let splits = match (last_time, time) {
                        (Some(prev), Some(now)) => now - prev > gap_ms,
                        _ => false,
                    };
                    if splits && burst.len() >= 2 {
                        push_or_flag(
                            burst.iter().map(|&i| candidates[i].hash.clone()).collect(),
                            bucket,
                            config,
                            &mut groups,
                            &mut oversize,
                        );
                        burst.clear();
                    } else if splits {
                        burst.clear();
                    }
                    burst.push(idx);
                    last_time = time.or(last_time);
                }
                if burst.len() >= 2 {
                    push_or_flag(
                        burst.iter().map(|&i| candidates[i].hash.clone()).collect(),
                        bucket,
                        config,
                        &mut groups,
                        &mut oversize,
                    );
                }
            }
        }
    }

    // Persist wholesale.
    conn.execute("DELETE FROM similar_group_members", [])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM similar_groups", [])
        .map_err(|e| e.to_string())?;

    let mut stats = GroupStats::default();
    for members in &groups {
        conn.execute(
            "INSERT INTO similar_groups (created_at_utc) VALUES (?1)",
            [crate::logging::now_iso_millis()],
        )
        .map_err(|e| e.to_string())?;
        let group_id = conn.last_insert_rowid();
        for hash in members {
            conn.execute(
                "INSERT INTO similar_group_members (group_id, content_hash) VALUES (?1, ?2)",
                params![group_id, hash],
            )
            .map_err(|e| e.to_string())?;
            stats.grouped_items += 1;
        }
        stats.groups += 1;
    }

    // Over-cap clusters: ungrouped, surfaced, never silently truncated.
    for (bucket, size) in &oversize {
        stats.oversize_clusters += 1;
        crate::logging::warn(
            "similar cluster over the size cap left ungrouped",
            serde_json::json!({ "bucket": bucket, "size": size, "cap": config.max_group_size }),
        );
        conn.execute(
            "INSERT INTO issues (path, kind, message, created_at_utc) VALUES (NULL, 'similar-overflow', ?1, ?2)",
            params![
                format!(
                    "{size} visually similar photos in {bucket} exceed the {} -photo group cap; \
                     lower the visual distance in Settings or raise the cap to compare them",
                    config.max_group_size
                ),
                crate::logging::now_iso_millis()
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(stats)
}

fn push_or_flag(
    members: Vec<String>,
    bucket: &str,
    config: &SimilarityConfig,
    groups: &mut Vec<Vec<String>>,
    oversize: &mut Vec<(String, usize)>,
) {
    if members.len() > config.max_group_size as usize {
        oversize.push((bucket.to_string(), members.len()));
    } else {
        groups.push(members);
    }
}

/// One group's members, best-first: sharpness descending (the advisory
/// machine guess), then time — never an auto-deletion criterion.
pub fn group_members(conn: &Connection, group_id: i64) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT m.content_hash FROM similar_group_members m \
             JOIN contents c ON c.hash = m.content_hash \
             WHERE m.group_id = ?1 \
             ORDER BY c.sharpness DESC NULLS LAST, c.hash",
        )
        .map_err(|e| e.to_string())?;
    let members = stmt
        .query_map([group_id], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_store;

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
}

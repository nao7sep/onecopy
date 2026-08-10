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

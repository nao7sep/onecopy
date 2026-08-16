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
//! Groups have NO size cap (the developer removed the earlier cap of 32,
//! 2026-08-16). The comparison view runs any group in 16-slot turns backed by
//! a queue, so a large family is several turns, not a hazard — and a cap was
//! worse than useless: an over-cap cluster was never persisted, so the LARGEST
//! families (the likeliest spares) silently got no group and no ≈ badge, while
//! filing a fresh issue on every rebuild. The tight default distance is what
//! keeps hairballs rare.
//!
//! Groups rebuild wholesale after each scan — membership is cheap to derive
//! and rebuilding sidesteps every incremental staleness bug. A group needs
//! ≥ 2 members to exist. Best-effort by design and documented as such; groups
//! order best-first by sharpness so slot 1 is the machine's guess.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

pub struct SimilarityConfig {
    pub max_gap_seconds: u32,
    pub phash_max_distance: u32,
    /// How much wider than one pairing step a family may spread (see the
    /// config field of the same name). Applied to BOTH signals: the dHash
    /// diameter allowance and, in angular terms, the embedding one.
    pub diameter_multiplier: u32,
    /// Cosine threshold for embedding pairing; None = disabled or no model —
    /// dHash-only, exactly the pre-embedding behaviour.
    pub embedding_min_cosine: Option<f32>,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct GroupStats {
    pub groups: u64,
    pub grouped_items: u64,
}

fn hamming(a: i64, b: i64) -> u32 {
    (a ^ b).count_ones()
}

/// Visual clustering within a bucket, with a BOUNDED DIAMETER.
///
/// Plain union-find components chain: A within distance of B and B of C puts
/// A and C in one "family" however far apart they are. On flat graphics —
/// icons, screenshots, renders, which crowd into a small corner of dhash
/// space — that chained a 75-member group whose farthest pair sat at distance
/// 28 with the threshold at 4: a ghost and a moon, "similar". Measured on the
/// developer's own index, 2026-08-16.
///
/// The repair keeps union-find for the well-behaved case and bounds the rest:
/// a component whose diameter is within TWICE the threshold stands whole (two
/// members of a real burst may sit `2d` apart through their shared middle),
/// and a wider component is re-clustered around LEADERS — each member joins
/// the first cluster whose leader is within the threshold, so no cluster's
/// diameter can exceed `2d` by construction. Members are ordered by hash
/// first, which keeps near-identical twins adjacent (they meet the same
/// leader) and makes the result deterministic.
pub fn cluster_by_appearance(
    phashes: &[i64],
    max_distance: u32,
    diameter_multiplier: u32,
) -> Vec<Vec<usize>> {
    let n = phashes.len();
    let mut uf = UnionFind::new(n);
    for a in 0..n {
        for b in (a + 1)..n {
            if hamming(phashes[a], phashes[b]) <= max_distance {
                uf.union(a, b);
            }
        }
    }
    let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = uf.find(i);
        components.entry(root).or_default().push(i);
    }

    let diameter_limit = max_distance * diameter_multiplier.max(1);
    let mut out: Vec<Vec<usize>> = Vec::new();
    for mut members in components.into_values() {
        members.sort_unstable();
        let chained = members.iter().enumerate().any(|(i, &a)| {
            members[(i + 1)..]
                .iter()
                .any(|&b| hamming(phashes[a], phashes[b]) > diameter_limit)
        });
        if !chained {
            out.push(members);
            continue;
        }
        members.sort_by_key(|&i| (phashes[i], i));
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        for &i in &members {
            match clusters
                .iter_mut()
                .find(|c| hamming(phashes[c[0]], phashes[i]) <= max_distance)
            {
                Some(cluster) => cluster.push(i),
                None => clusters.push(vec![i]),
            }
        }
        out.extend(clusters);
    }
    // Deterministic output order regardless of HashMap iteration.
    out.sort_by_key(|c| c[0]);
    out
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
    embedding: Option<Vec<f32>>,
}

/// Rebuilds every similar group from the current index. Only images with a
/// perceptual hash participate; the logical item's time is the earliest among
/// its copies (the same rule the sections use), and undated images form their
/// own bucket rather than being excluded.
/// Canonical form of an exclusion pair: lexicographic, so one row (and one
/// set entry) represents "a and b are not the same subject" regardless of
/// which side the user unlinked from.
fn canonical_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Every excluded pair, canonical, for the rebuild to honour.
fn load_exclusions(
    conn: &Connection,
) -> Result<std::collections::HashSet<(String, String)>, String> {
    let mut stmt = conn
        .prepare("SELECT hash_a, hash_b FROM similar_exclusions")
        .map_err(|e| e.to_string())?;
    let pairs = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(pairs)
}

/// Enforces the user's unlink verdicts on a finished group list: no group may
/// contain an excluded pair. Applied AFTER both clustering stages rather than
/// inside them, deliberately — an edge skipped during union-find still joins
/// its endpoints through a middleman, so pairwise enforcement during
/// clustering is a lie. Greedy: members keep their order and each lands in
/// the first subset holding nobody it is excluded against, so the common case
/// — one intruder unlinked against a whole family — costs exactly that
/// intruder, and the family stands whole.
pub fn split_by_exclusions(
    groups: Vec<Vec<String>>,
    excluded: &std::collections::HashSet<(String, String)>,
) -> Vec<Vec<String>> {
    if excluded.is_empty() {
        return groups;
    }
    let mut out: Vec<Vec<String>> = Vec::new();
    for members in groups {
        let conflicted = members.iter().enumerate().any(|(i, a)| {
            members[(i + 1)..]
                .iter()
                .any(|b| excluded.contains(&canonical_pair(a, b)))
        });
        if !conflicted {
            out.push(members);
            continue;
        }
        let mut subsets: Vec<Vec<String>> = Vec::new();
        for member in members {
            match subsets.iter_mut().find(|subset| {
                subset
                    .iter()
                    .all(|other| !excluded.contains(&canonical_pair(&member, other)))
            }) {
                Some(subset) => subset.push(member),
                None => subsets.push(vec![member]),
            }
        }
        out.extend(subsets.into_iter().filter(|subset| subset.len() >= 2));
    }
    out
}

/// The comparison view's unlink: this image is NOT the same subject as its
/// similar-family. Writes one exclusion per other CURRENT member (a fact
/// about the images, so it survives the wholesale group rebuilds), removes
/// the membership row for immediate effect, and dissolves the group when
/// fewer than two members remain. Returns how many exclusions were recorded.
pub fn unlink_from_group(conn: &Connection, hash: &str) -> Result<u64, String> {
    let group_id: Option<i64> = conn
        .query_row(
            "SELECT group_id FROM similar_group_members WHERE content_hash = ?1",
            [hash],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(group_id) = group_id else {
        return Ok(0);
    };
    let others: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT content_hash FROM similar_group_members                  WHERE group_id = ?1 AND content_hash != ?2",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params![group_id, hash], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        rows
    };
    let now = crate::logging::now_iso_millis();
    let mut written = 0u64;
    for other in &others {
        let (a, b) = canonical_pair(hash, other);
        written += conn
            .execute(
                "INSERT OR IGNORE INTO similar_exclusions (hash_a, hash_b, created_at_utc)                  VALUES (?1, ?2, ?3)",
                rusqlite::params![a, b, now],
            )
            .map_err(|e| e.to_string())? as u64;
    }
    conn.execute(
        "DELETE FROM similar_group_members WHERE group_id = ?1 AND content_hash = ?2",
        rusqlite::params![group_id, hash],
    )
    .map_err(|e| e.to_string())?;
    let remaining: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM similar_group_members WHERE group_id = ?1",
            [group_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if remaining < 2 {
        conn.execute(
            "DELETE FROM similar_group_members WHERE group_id = ?1",
            [group_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM similar_groups WHERE id = ?1", [group_id])
            .map_err(|e| e.to_string())?;
    }
    crate::logging::info(
        "similar unlink",
        serde_json::json!({ "hash": hash, "exclusions": written, "groupDissolved": remaining < 2 }),
    );
    Ok(written)
}

pub fn rebuild_groups(
    conn: &Connection,
    config: &SimilarityConfig,
) -> Result<GroupStats, String> {
    let read_embeddings = config.embedding_min_cosine.is_some();
    let mut stmt = conn
        .prepare(
            "SELECT c.hash, \
             COALESCE(c.camera_make, '') || '|' || COALESCE(c.camera_model, ''), \
             MIN(p.resolved_utc_ms), c.phash, c.embedding \
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
                embedding: if read_embeddings {
                    r.get::<_, Option<Vec<u8>>>(4)?
                        .and_then(|blob| crate::embedding::from_blob(&blob))
                } else {
                    None
                },
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

    for indices in buckets.values() {
        // Quadratic within the bucket — integer work over in-memory rows, no
        // file reads. Chained components split around leaders (see
        // cluster_by_appearance).
        let mut bucket_groups: Vec<Vec<usize>> = Vec::new();
        let phashes: Vec<i64> = indices.iter().map(|&i| candidates[i].phash).collect();
        for cluster in cluster_by_appearance(
            &phashes,
            config.phash_max_distance,
            config.diameter_multiplier,
        ) {
            let cluster: Vec<usize> = cluster.into_iter().map(|local| indices[local]).collect();
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
                        bucket_groups.push(members.clone());
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
                        bucket_groups.push(burst.clone());
                        burst.clear();
                    } else if splits {
                        burst.clear();
                    }
                    burst.push(idx);
                    last_time = time.or(last_time);
                }
                if burst.len() >= 2 {
                    bucket_groups.push(burst.clone());
                }
            }
        }

        // Stage B — cross-device pairing (Design: Similar-shot grouping):
        // deliberately crosses camera partitions and skips the burst split,
        // because different devices' clocks disagree and appearance is the
        // only trustworthy signal there.
        let merged = if let Some(min_cosine) = config.embedding_min_cosine {
            merge_by_embedding(bucket_groups, indices, &candidates, min_cosine)
        } else {
            bucket_groups
        };
        for group in merged {
            if group.len() >= 2 {
                groups.push(group.iter().map(|&i| candidates[i].hash.clone()).collect());
            }
        }
    }

    // The user's unlink verdicts bind every rebuild, not just the session
    // they were made in.
    let groups = split_by_exclusions(groups, &load_exclusions(conn)?);

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

    Ok(stats)
}

/// Merges visual groups through cross-group clusters (the embedding stage):
/// each cluster unions the groups its members belong to — members in no
/// group join as singletons — and groups no cluster touches pass through.
/// Stage B's merge: visual groups and ungrouped loners are UNITS, and units
/// join a merged cluster only when their representative sits within the
/// cosine threshold OF THAT CLUSTER'S LEADER — the same leader-bounded
/// discipline `cluster_by_appearance` applies to phashes, for exactly the
/// same reason.
///
/// The previous form unioned any two groups that shared an embedding cluster,
/// transitively. Leader-bounded CLUSTERS cannot chain, but a union OVER them
/// can and did: with unrelated app icons reaching the threshold in about one
/// pair in seventeen, that was enough connectivity to fuse 207 icons —
/// butterflies, microphones, cassettes — into a single "similar" group whose
/// median pair sat at cosine 0.72 and dhash distance 28 (measured on the
/// developer's index, 2026-08-17). A false pair must cost one wrong neighbour,
/// never the whole bucket.
fn merge_by_embedding(
    groups: Vec<Vec<usize>>,
    indices: &[usize],
    candidates: &[Candidate],
    min_cosine: f32,
) -> Vec<Vec<usize>> {
    use std::collections::HashSet;
    let grouped: HashSet<usize> = groups.iter().flatten().copied().collect();
    // Units, in a deterministic order: every visual group, then every loner.
    let mut units: Vec<Vec<usize>> = groups;
    for &i in indices {
        if !grouped.contains(&i) {
            units.push(vec![i]);
        }
    }
    // A unit's representative is its first member carrying an embedding. A
    // visual group is already tight in dhash space, so any member represents
    // it; a unit with no embedding at all cannot merge and stands as it is.
    let reps: Vec<Option<Vec<f32>>> = units
        .iter()
        .map(|unit| {
            unit.iter()
                .find_map(|&i| candidates[i].embedding.clone())
        })
        .collect();

    let mut merged: Vec<Vec<usize>> = Vec::new();
    let mut consumed = vec![false; units.len()];
    for cluster in crate::embedding::embedding_clusters(&reps, min_cosine) {
        let mut members: Vec<usize> = Vec::new();
        for unit in cluster {
            consumed[unit] = true;
            members.extend(units[unit].iter().copied());
        }
        merged.push(members);
    }
    for (unit, used) in consumed.iter().enumerate() {
        if !used {
            merged.push(std::mem::take(&mut units[unit]));
        }
    }
    merged
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

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
//! Month buckets bound memory and product scope. Inside them, disjoint Hamming
//! bands produce exact candidates for the strict visual threshold, while a
//! sliding capture-time window covers the wider burst threshold; full dHash
//! checks remove false candidates. A family straddling a month boundary still
//! splits — accepted by design. Bucket months are UTC — a scope, not a display
//! concept; section display months can differ near boundaries by the
//! display-timezone offset.
//!
//! Groups have NO size cap (the developer removed the earlier cap of 32,
//! 2026-08-16). The comparison view runs any group in 16-slot turns backed by
//! a queue, so a large family is several turns, not a hazard — and a cap was
//! worse than useless: an over-cap cluster was never persisted, so the LARGEST
//! families (the likeliest spares) silently got no group and no ≈ badge, while
//! filing a fresh issue on every rebuild. The tight default distance is what
//! keeps hairballs rare.
//!
//! Each bucket publishes as one complete cohort. Source-fact triggers retain
//! a durable revision for only the affected buckets; a rebuild computed from
//! an older revision cannot replace the published cohort or clear the newer
//! invalidation. A group needs ≥ 2 members to exist. Groups order best-first
//! by sharpness so slot 1 is the machine's guess.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

pub struct SimilarityConfig {
    pub max_gap_seconds: u32,
    pub phash_max_distance: u32,
    /// The RELAXED distance for pairs whose capture times sit within
    /// `max_gap_seconds` of each other (Phase 33). Family bursts legitimately
    /// spread wider in dhash than the strict threshold tolerates — handheld
    /// shift, subject motion, a toddler mid-turn — and capture time is the
    /// strongest, cheapest signal that two frames belong together. Pairs far
    /// apart in time (or with no time at all) keep the strict distance, so
    /// flat art and icons are exactly as hard to group as before.
    pub phash_max_distance_burst: u32,
    /// How much wider than one pairing step a family may spread (see the
    /// config field of the same name), as a dHash diameter allowance.
    pub diameter_multiplier: u32,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct GroupStats {
    pub groups: u64,
    pub grouped_items: u64,
    pub buckets: u64,
    pub last_bucket: Option<String>,
}

fn hamming(a: i64, b: i64) -> u32 {
    (a ^ b).count_ones()
}

/// Exact strict-distance candidate generation for 64-bit dHash. Splitting the
/// bits into `distance + 1` disjoint bands gives the pigeonhole guarantee: two
/// hashes at Hamming distance <= `distance` share at least one whole band.
/// Buckets may produce false positives, which the full-distance check removes;
/// they cannot miss a qualifying pair.
fn union_strict_neighbors(
    phashes: &[i64],
    distance: u32,
    uf: &mut UnionFind,
    comparisons: &mut usize,
    stop: &dyn Fn() -> bool,
) -> Result<(), String> {
    if distance >= 64 {
        for a in 0..phashes.len() {
            for b in (a + 1)..phashes.len() {
                checked_comparison(comparisons, stop)?;
                uf.union(a, b);
            }
        }
        return Ok(());
    }

    let band_count = distance as usize + 1;
    let mut buckets: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
    let mut seen_at = vec![usize::MAX; phashes.len()];
    for a in 0..phashes.len() {
        if a % 1024 == 0 && stop() {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        for band in 0..band_count {
            let key = (band, hamming_band(phashes[a] as u64, band, band_count));
            if let Some(candidates) = buckets.get(&key) {
                for &b in candidates {
                    if seen_at[b] == a {
                        continue;
                    }
                    seen_at[b] = a;
                    checked_comparison(comparisons, stop)?;
                    if hamming(phashes[a], phashes[b]) <= distance {
                        uf.union(a, b);
                    }
                }
            }
        }
        for band in 0..band_count {
            let key = (band, hamming_band(phashes[a] as u64, band, band_count));
            buckets.entry(key).or_default().push(a);
        }
    }
    Ok(())
}

fn hamming_band(hash: u64, band: usize, band_count: usize) -> u64 {
    let base_width = 64 / band_count;
    let wider_bands = 64 % band_count;
    let width = base_width + usize::from(band < wider_bands);
    let offset = band * base_width + band.min(wider_bands);
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    (hash >> offset) & mask
}

/// The relaxed threshold applies only to pairs close in capture time. Sorting
/// known timestamps turns that rule into a sliding window; undated or distant
/// pairs were already covered exactly by the strict Hamming search.
fn union_burst_neighbors(
    phashes: &[i64],
    times_ms: &[Option<i64>],
    strict_distance: u32,
    burst_distance: u32,
    gap_ms: i64,
    uf: &mut UnionFind,
    comparisons: &mut usize,
    stop: &dyn Fn() -> bool,
) -> Result<(), String> {
    let distance = strict_distance.max(burst_distance);
    if distance <= strict_distance {
        return Ok(());
    }
    let mut timed: Vec<(i64, usize)> = times_ms
        .iter()
        .enumerate()
        .filter_map(|(index, time)| time.map(|value| (value, index)))
        .collect();
    timed.sort_unstable();
    let mut left = 0usize;
    for right in 0..timed.len() {
        if right % 1024 == 0 && stop() {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        while timed[right].0 - timed[left].0 > gap_ms {
            left += 1;
        }
        for prior in left..right {
            checked_comparison(comparisons, stop)?;
            let a = timed[prior].1;
            let b = timed[right].1;
            if hamming(phashes[a], phashes[b]) <= distance {
                uf.union(a, b);
            }
        }
    }
    Ok(())
}

fn checked_comparison(comparisons: &mut usize, stop: &dyn Fn() -> bool) -> Result<(), String> {
    *comparisons += 1;
    if *comparisons % 1024 == 0 && stop() {
        Err(crate::scanner::CANCELLED.to_string())
    } else {
        Ok(())
    }
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
    times_ms: &[Option<i64>],
    strict_distance: u32,
    burst_distance: u32,
    burst_gap_seconds: u32,
    diameter_multiplier: u32,
) -> Result<Vec<Vec<usize>>, String> {
    cluster_by_appearance_cancellable(
        phashes,
        times_ms,
        strict_distance,
        burst_distance,
        burst_gap_seconds,
        diameter_multiplier,
        &|| false,
    )
}

fn cluster_by_appearance_cancellable(
    phashes: &[i64],
    times_ms: &[Option<i64>],
    strict_distance: u32,
    burst_distance: u32,
    burst_gap_seconds: u32,
    diameter_multiplier: u32,
    stop: &dyn Fn() -> bool,
) -> Result<Vec<Vec<usize>>, String> {
    let n = phashes.len();
    debug_assert_eq!(n, times_ms.len());
    let gap_ms = i64::from(burst_gap_seconds) * 1000;
    // The per-pair allowance remains the diameter/refinement rule after exact
    // candidate generation has built the connected components.
    let allowed = |a: usize, b: usize| -> u32 {
        match (times_ms[a], times_ms[b]) {
            (Some(ta), Some(tb)) if (ta - tb).abs() <= gap_ms => {
                strict_distance.max(burst_distance)
            }
            _ => strict_distance,
        }
    };
    let mut uf = UnionFind::new(n);
    let mut comparisons = 0usize;
    union_strict_neighbors(phashes, strict_distance, &mut uf, &mut comparisons, stop)?;
    union_burst_neighbors(
        phashes,
        times_ms,
        strict_distance,
        burst_distance,
        gap_ms,
        &mut uf,
        &mut comparisons,
        stop,
    )?;
    let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = uf.find(i);
        components.entry(root).or_default().push(i);
    }

    let mut out: Vec<Vec<usize>> = Vec::new();
    for mut members in components.into_values() {
        members.sort_unstable();
        // The diameter discipline survives the gating per pair: two members
        // admitted through the burst allowance may spread to its multiple,
        // two admitted through the strict one only to the strict multiple.
        let chained = members.iter().enumerate().any(|(i, &a)| {
            members[(i + 1)..].iter().any(|&b| {
                hamming(phashes[a], phashes[b]) > allowed(a, b) * diameter_multiplier.max(1)
            })
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
                .find(|c| hamming(phashes[c[0]], phashes[i]) <= allowed(c[0], i))
            {
                Some(cluster) => cluster.push(i),
                None => clusters.push(vec![i]),
            }
        }
        out.extend(clusters);
    }
    // Deterministic output order regardless of HashMap iteration.
    out.sort_by_key(|c| c[0]);
    Ok(out)
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

const DATED_CANDIDATES_SQL: &str = "SELECT c.hash,
            COALESCE(c.camera_make, '') || '|' || COALESCE(c.camera_model, ''),
            l.resolved_utc_ms, c.phash
     FROM logical_contents l
     JOIN contents c ON c.hash = l.content_hash
     WHERE l.kind = 'image' AND l.resolved_utc_ms >= ?1
       AND l.resolved_utc_ms < ?2 AND c.phash IS NOT NULL";

const UNDATED_CANDIDATES_SQL: &str = "SELECT c.hash,
            COALESCE(c.camera_make, '') || '|' || COALESCE(c.camera_model, ''),
            l.resolved_utc_ms, c.phash
     FROM logical_contents l
     JOIN contents c ON c.hash = l.content_hash
     WHERE l.kind = 'image' AND l.resolved_utc_ms IS NULL
       AND c.phash IS NOT NULL";

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
/// about the images, so it survives every cohort rebuild), removes
/// the membership row for immediate effect, and dissolves the group when
/// fewer than two members remain. Returns how many exclusions were recorded.
pub fn unlink_from_group(
    conn: &Connection,
    root: &std::path::Path,
    hash: &str,
) -> Result<u64, String> {
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
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    // Bracket the authored-store write with durable invalidation. A rebuild
    // racing the first marker may still read the old verdicts, while the
    // second marker and stored fingerprint guarantee another pass sees the
    // new set. An authored write followed by an index failure is recovered by
    // the fingerprint comparison on the next worker turn.
    mark_hash_bucket_dirty(conn, hash)?;
    let written = crate::similar_exclusions::add_for_peers(root, hash, &others)?;
    let exclusions = crate::similar_exclusions::pairs(root)?;
    record_targeted_exclusions_change(conn, hash, &exclusions)?;
    let transaction = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    transaction
        .execute(
            "DELETE FROM similar_group_members WHERE group_id = ?1 AND content_hash = ?2",
            rusqlite::params![group_id, hash],
        )
        .map_err(|e| e.to_string())?;
    let remaining: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM similar_group_members WHERE group_id = ?1",
            [group_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if remaining < 2 {
        transaction
            .execute(
                "DELETE FROM similar_group_members WHERE group_id = ?1",
                [group_id],
            )
            .map_err(|e| e.to_string())?;
        transaction
            .execute("DELETE FROM similar_groups WHERE id = ?1", [group_id])
            .map_err(|e| e.to_string())?;
    }
    transaction.commit().map_err(|e| e.to_string())?;
    crate::logging::info(
        "similar unlink",
        serde_json::json!({ "hash": hash, "exclusions": written, "groupDissolved": remaining < 2 }),
    );
    Ok(written)
}

fn config_fingerprint(config: &SimilarityConfig) -> String {
    format!(
        "{}:{}:{}:{}",
        config.max_gap_seconds,
        config.phash_max_distance,
        config.phash_max_distance_burst,
        config.diameter_multiplier
    )
}

fn exclusions_fingerprint(exclusions: &std::collections::HashSet<(String, String)>) -> String {
    let mut pairs = exclusions.iter().collect::<Vec<_>>();
    pairs.sort_unstable();
    let mut hasher = blake3::Hasher::new();
    for (left, right) in pairs {
        hasher.update(&(left.len() as u64).to_le_bytes());
        hasher.update(left.as_bytes());
        hasher.update(&(right.len() as u64).to_le_bytes());
        hasher.update(right.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn dirty_bucket_expression(alias: &str) -> String {
    format!("COALESCE(strftime('%Y-%m', {alias}.resolved_utc_ms / 1000.0, 'unixepoch'), 'undated')")
}

fn mark_all_buckets_dirty_in(conn: &Connection) -> Result<(), String> {
    let bucket = dirty_bucket_expression("l");
    conn.execute_batch(&format!(
        "INSERT INTO similarity_dirty_buckets (bucket, revision)
         SELECT bucket, 1
         FROM (
           SELECT {bucket} AS bucket
           FROM logical_contents l
           WHERE l.kind = 'image'
           UNION
           SELECT bucket FROM similar_groups
         )
         WHERE 1
         ON CONFLICT(bucket) DO UPDATE SET revision = revision + 1"
    ))
    .map_err(|error| error.to_string())
}

pub fn mark_all_buckets_dirty(conn: &Connection) -> Result<(), String> {
    let transaction = rusqlite::Transaction::new_unchecked(
        conn,
        rusqlite::TransactionBehavior::Immediate,
    )
    .map_err(|error| error.to_string())?;
    mark_all_buckets_dirty_in(&transaction)?;
    transaction.commit().map_err(|error| error.to_string())
}

fn mark_hash_bucket_dirty(conn: &Connection, hash: &str) -> Result<(), String> {
    let bucket = dirty_bucket_expression("l");
    conn.execute(
        &format!(
            "INSERT INTO similarity_dirty_buckets (bucket, revision)
             SELECT {bucket}, 1
             FROM logical_contents l
             WHERE l.content_hash = ?1 AND l.kind = 'image'
             ON CONFLICT(bucket) DO UPDATE SET revision = revision + 1"
        ),
        [hash],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn dirty_bucket_count(conn: &Connection) -> Result<u64, String> {
    conn.query_row("SELECT COUNT(*) FROM similarity_dirty_buckets", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|count| count.max(0) as u64)
    .map_err(|error| error.to_string())
}

pub fn ensure_config_current(conn: &Connection, config: &SimilarityConfig) -> Result<(), String> {
    let fingerprint = config_fingerprint(config);
    let current = conn
        .query_row(
            "SELECT config_fingerprint FROM similarity_state WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if current.as_deref() == Some(&fingerprint) {
        return Ok(());
    }

    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    mark_all_buckets_dirty_in(&transaction)?;
    transaction
        .execute(
            "INSERT INTO similarity_state (singleton, config_fingerprint)
             VALUES (1, ?1)
             ON CONFLICT(singleton) DO UPDATE
             SET config_fingerprint = excluded.config_fingerprint",
            [&fingerprint],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn ensure_exclusions_current(
    conn: &Connection,
    exclusions: &std::collections::HashSet<(String, String)>,
) -> Result<(), String> {
    let fingerprint = exclusions_fingerprint(exclusions);
    let current = conn
        .query_row(
            "SELECT exclusions_fingerprint FROM similarity_state WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if current.as_deref() == Some(&fingerprint) {
        return Ok(());
    }
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    mark_all_buckets_dirty_in(&transaction)?;
    transaction
        .execute(
            "UPDATE similarity_state SET exclusions_fingerprint = ?1 WHERE singleton = 1",
            [&fingerprint],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn record_targeted_exclusions_change(
    conn: &Connection,
    hash: &str,
    exclusions: &std::collections::HashSet<(String, String)>,
) -> Result<(), String> {
    let fingerprint = exclusions_fingerprint(exclusions);
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    mark_hash_bucket_dirty(&transaction, hash)?;
    transaction
        .execute(
            "UPDATE similarity_state SET exclusions_fingerprint = ?1 WHERE singleton = 1",
            [&fingerprint],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_all_exclusions_change(
    conn: &Connection,
    exclusions: &std::collections::HashSet<(String, String)>,
) -> Result<(), String> {
    let fingerprint = exclusions_fingerprint(exclusions);
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    mark_all_buckets_dirty_in(&transaction)?;
    transaction
        .execute(
            "UPDATE similarity_state SET exclusions_fingerprint = ?1 WHERE singleton = 1",
            [&fingerprint],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

fn bucket_bounds(bucket: &str) -> Result<Option<(i64, i64)>, String> {
    if bucket == "undated" {
        return Ok(None);
    }
    let first = chrono::NaiveDate::parse_from_str(&format!("{bucket}-01"), "%Y-%m-%d")
        .map_err(|_| format!("invalid similarity bucket {bucket}"))?;
    let next = first
        .checked_add_months(chrono::Months::new(1))
        .ok_or_else(|| format!("invalid similarity bucket {bucket}"))?;
    let at_midnight = |date: chrono::NaiveDate| {
        date.and_hms_opt(0, 0, 0)
            .map(|value| value.and_utc().timestamp_millis())
            .ok_or_else(|| format!("invalid similarity bucket {bucket}"))
    };
    Ok(Some((at_midnight(first)?, at_midnight(next)?)))
}

fn candidates_for_bucket(conn: &Connection, bucket: &str) -> Result<Vec<Candidate>, String> {
    let collect = |mut stmt: rusqlite::Statement<'_>, parameters: &[&dyn rusqlite::ToSql]| {
        let rows = stmt
            .query_map(parameters, |row| {
                Ok(Candidate {
                    hash: row.get(0)?,
                    camera: row.get(1)?,
                    time_ms: row.get(2)?,
                    phash: row.get(3)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())
    };
    match bucket_bounds(bucket)? {
        Some((start, end)) => collect(
            conn.prepare(DATED_CANDIDATES_SQL)
                .map_err(|error| error.to_string())?,
            &[&start, &end],
        ),
        None => collect(
            conn.prepare(UNDATED_CANDIDATES_SQL)
                .map_err(|error| error.to_string())?,
            &[],
        ),
    }
}

fn groups_for_bucket(
    candidates: &[Candidate],
    config: &SimilarityConfig,
    exclusions: &std::collections::HashSet<(String, String)>,
    stop: &dyn Fn() -> bool,
) -> Result<Vec<Vec<String>>, String> {
    if stop() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    let phashes = candidates
        .iter()
        .map(|candidate| candidate.phash)
        .collect::<Vec<_>>();
    let times = candidates
        .iter()
        .map(|candidate| candidate.time_ms)
        .collect::<Vec<_>>();
    let gap_ms = i64::from(config.max_gap_seconds) * 1000;
    let mut groups = Vec::new();
    for cluster in cluster_by_appearance_cancellable(
        &phashes,
        &times,
        config.phash_max_distance,
        config.phash_max_distance_burst,
        config.max_gap_seconds,
        config.diameter_multiplier,
        stop,
    )? {
        if cluster.len() < 2 {
            continue;
        }
        let mut by_camera: HashMap<&str, Vec<usize>> = HashMap::new();
        for index in cluster {
            by_camera
                .entry(candidates[index].camera.as_str())
                .or_default()
                .push(index);
        }
        for (camera, mut members) in by_camera {
            if camera == "|" {
                if members.len() >= 2 {
                    groups.push(
                        members
                            .into_iter()
                            .map(|index| candidates[index].hash.clone())
                            .collect(),
                    );
                }
                continue;
            }
            members.sort_by_key(|&index| candidates[index].time_ms.unwrap_or(i64::MAX));
            let mut burst: Vec<usize> = Vec::new();
            let mut last_time: Option<i64> = None;
            for index in members {
                let time = candidates[index].time_ms;
                let splits = matches!((last_time, time), (Some(previous), Some(now)) if now - previous > gap_ms);
                if splits {
                    if burst.len() >= 2 {
                        groups.push(
                            burst
                                .drain(..)
                                .map(|member| candidates[member].hash.clone())
                                .collect(),
                        );
                    } else {
                        burst.clear();
                    }
                }
                burst.push(index);
                last_time = time.or(last_time);
            }
            if burst.len() >= 2 {
                groups.push(
                    burst
                        .into_iter()
                        .map(|index| candidates[index].hash.clone())
                        .collect(),
                );
            }
        }
    }
    Ok(split_by_exclusions(groups, exclusions))
}

fn publish_bucket(
    conn: &Connection,
    bucket: &str,
    claimed_revision: i64,
    groups: &[Vec<String>],
    stop: &dyn Fn() -> bool,
) -> Result<Option<GroupStats>, String> {
    if stop() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    let transaction = rusqlite::Transaction::new_unchecked(
        conn,
        rusqlite::TransactionBehavior::Immediate,
    )
    .map_err(|error| error.to_string())?;
    let current_revision = transaction
        .query_row(
            "SELECT revision FROM similarity_dirty_buckets WHERE bucket = ?1",
            [bucket],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if current_revision != Some(claimed_revision) {
        return Ok(None);
    }

    transaction
        .execute(
            "DELETE FROM similar_group_members
             WHERE group_id IN (SELECT id FROM similar_groups WHERE bucket = ?1)",
            [bucket],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM similar_groups WHERE bucket = ?1", [bucket])
        .map_err(|error| error.to_string())?;

    let mut stats = GroupStats {
        buckets: 1,
        last_bucket: Some(bucket.to_string()),
        ..GroupStats::default()
    };
    let created_at = crate::logging::now_iso_millis();
    let mut inserted = 0usize;
    for members in groups {
        if stop() {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        transaction
            .execute(
                "INSERT INTO similar_groups (bucket, created_at_utc) VALUES (?1, ?2)",
                params![bucket, created_at],
            )
            .map_err(|error| error.to_string())?;
        let group_id = transaction.last_insert_rowid();
        for hash in members {
            inserted += 1;
            if inserted % 1024 == 0 && stop() {
                return Err(crate::scanner::CANCELLED.to_string());
            }
            transaction
                .execute(
                    "INSERT INTO similar_group_members (group_id, content_hash) VALUES (?1, ?2)",
                    params![group_id, hash],
                )
                .map_err(|error| error.to_string())?;
            stats.grouped_items += 1;
        }
        stats.groups += 1;
    }
    transaction
        .execute(
            "DELETE FROM similarity_dirty_buckets WHERE bucket = ?1 AND revision = ?2",
            params![bucket, claimed_revision],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(Some(stats))
}

fn next_dirty_bucket(conn: &Connection) -> Result<Option<(String, i64)>, String> {
    conn.query_row(
        "SELECT bucket, revision FROM similarity_dirty_buckets
         ORDER BY bucket = 'undated', bucket LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn rebuild_next_dirty_bucket_with_exclusions(
    conn: &Connection,
    config: &SimilarityConfig,
    exclusions: &std::collections::HashSet<(String, String)>,
    stop: &dyn Fn() -> bool,
) -> Result<Option<GroupStats>, String> {
    loop {
        let Some((bucket, revision)) = next_dirty_bucket(conn)? else {
            return Ok(None);
        };
        let candidates = candidates_for_bucket(conn, &bucket)?;
        let groups = groups_for_bucket(&candidates, config, exclusions, stop)?;
        if let Some(stats) = publish_bucket(conn, &bucket, revision, &groups, stop)? {
            return Ok(Some(stats));
        }
    }
}

fn rebuild_all_dirty(
    conn: &Connection,
    config: &SimilarityConfig,
    exclusions: &std::collections::HashSet<(String, String)>,
    stop: &dyn Fn() -> bool,
) -> Result<GroupStats, String> {
    let mut total = GroupStats::default();
    while let Some(stats) =
        rebuild_next_dirty_bucket_with_exclusions(conn, config, exclusions, stop)?
    {
        total.groups += stats.groups;
        total.grouped_items += stats.grouped_items;
        total.buckets += stats.buckets;
        total.last_bucket = stats.last_bucket;
    }
    Ok(total)
}

pub fn rebuild_groups(conn: &Connection, config: &SimilarityConfig) -> Result<GroupStats, String> {
    ensure_config_current(conn, config)?;
    mark_all_buckets_dirty(conn)?;
    rebuild_all_dirty(conn, config, &std::collections::HashSet::new(), &|| false)
}

pub fn rebuild_groups_for_root(
    conn: &Connection,
    config: &SimilarityConfig,
    root: &std::path::Path,
) -> Result<GroupStats, String> {
    ensure_config_current(conn, config)?;
    mark_all_buckets_dirty(conn)?;
    let exclusions = crate::similar_exclusions::pairs(root)?;
    ensure_exclusions_current(conn, &exclusions)?;
    rebuild_all_dirty(conn, config, &exclusions, &|| false)
}

pub fn rebuild_next_dirty_bucket_for_root_cancellable(
    conn: &Connection,
    config: &SimilarityConfig,
    root: &std::path::Path,
    stop: &dyn Fn() -> bool,
) -> Result<Option<GroupStats>, String> {
    crate::resource_limits::require_available(
        crate::resource_limits::SIMILARITY_REQUIRED_AVAILABLE,
        "Similarity analysis",
    )?;
    ensure_config_current(conn, config)?;
    let exclusions = crate::similar_exclusions::pairs(root)?;
    ensure_exclusions_current(conn, &exclusions)?;
    rebuild_next_dirty_bucket_with_exclusions(conn, config, &exclusions, stop)
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
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(members)
}

// EXCEPTION (tests-folder convention): this pins the private candidate SQL
// used by the shipped rebuild rather than duplicating it in an integration
// test that could silently diverge.
#[cfg(test)]
mod candidate_query_tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn dated_candidate_load_seeks_one_logical_month_without_regrouping_paths() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-similarity-plan-")
            .tempdir()
            .unwrap();
        let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let mut statement = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {DATED_CANDIDATES_SQL}"))
            .unwrap();
        let details: Vec<String> = statement
            .query_map([1_704_067_200_000i64, 1_706_745_600_000i64], |row| {
                row.get(3)
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert!(
            details.iter().any(|line| line.contains("logical_contents")),
            "similarity lost the maintained logical source: {details:?}"
        );
        assert!(
            details.iter().any(|line| line.contains("resolved_utc_ms")),
            "similarity no longer seeks the requested UTC month: {details:?}"
        );
        assert!(
            details.iter().all(|line| !line.contains("paths")),
            "similarity regressed to physical-path grouping: {details:?}"
        );
        assert!(
            details.iter().all(|line| !line.contains("TEMP B-TREE")),
            "similarity candidate loading reintroduced sorting/grouping: {details:?}"
        );
    }

    #[test]
    fn sparse_candidate_construction_remains_cancellable() {
        let phashes: Vec<i64> = (0..10_000).map(|value| value * 0x1_0001).collect();
        let times = vec![None; phashes.len()];
        let polls = Cell::new(0usize);
        let stopped = || {
            polls.set(polls.get() + 1);
            polls.get() >= 3
        };

        let error = cluster_by_appearance_cancellable(&phashes, &times, 4, 10, 90, 2, &stopped)
            .unwrap_err();
        assert_eq!(error, crate::scanner::CANCELLED);
        assert!(polls.get() >= 3);
    }
}

//! Similar-shot grouping — the two-stage design: a TIME GATE chains photos
//! from the same camera whose consecutive gaps stay within the configured
//! window (the deliberate spare-shot pattern), then a VISUAL SPLIT breaks a
//! chain where the perceptual-hash distance jumps (a genuine scene change
//! inside the window). Best-effort by design and documented as such; groups
//! order best-first by sharpness so slot 1 is the machine's guess.
//!
//! Groups are computed globally and rebuilt wholesale after each scan —
//! membership is cheap to derive and rebuilding sidesteps every incremental
//! staleness bug. A group needs ≥ 2 members to exist.

use rusqlite::{params, Connection};

pub struct SimilarityConfig {
    pub max_gap_seconds: u32,
    pub phash_max_distance: u32,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct GroupStats {
    pub groups: u64,
    pub grouped_items: u64,
}

fn hamming(a: i64, b: i64) -> u32 {
    (a ^ b).count_ones()
}

/// Rebuilds every similar group from the current index. Only images with a
/// resolved time and a perceptual hash participate; the logical item's time is
/// the earliest among its copies (the same rule the sections use).
pub fn rebuild_groups(
    conn: &Connection,
    config: &SimilarityConfig,
) -> Result<GroupStats, String> {
    // Candidate logical items: hash, camera identity, time, phash.
    let mut stmt = conn
        .prepare(
            "SELECT c.hash, \
             COALESCE(c.camera_make, '') || '|' || COALESCE(c.camera_model, ''), \
             MIN(p.resolved_utc_ms), c.phash \
             FROM contents c JOIN paths p ON p.content_hash = c.hash \
             WHERE c.kind = 'image' AND c.phash IS NOT NULL \
               AND p.missing = 0 AND p.companion_of IS NULL \
               AND p.resolved_utc_ms IS NOT NULL \
             GROUP BY c.hash",
        )
        .map_err(|e| e.to_string())?;
    let mut candidates: Vec<(String, String, i64, i64)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    // Chain per camera along time.
    candidates.sort_by(|a, b| (&a.1, a.2).cmp(&(&b.1, b.2)));

    let gap_ms = i64::from(config.max_gap_seconds) * 1000;
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<(String, i64)> = Vec::new(); // (hash, phash) of the open group
    let mut last_camera: Option<String> = None;
    let mut last_time: i64 = 0;

    let flush = |current: &mut Vec<(String, i64)>, groups: &mut Vec<Vec<String>>| {
        if current.len() >= 2 {
            groups.push(current.iter().map(|(h, _)| h.clone()).collect());
        }
        current.clear();
    };

    for (hash, camera, time, phash) in candidates {
        let same_camera = last_camera.as_deref() == Some(camera.as_str());
        let within_gap = same_camera && (time - last_time) <= gap_ms;
        // Visual check against the LAST member: a burst drifts gradually, so
        // neighbor distance is the honest comparator, not the group's first.
        let visually_close = current
            .last()
            .map(|(_, last_hash)| hamming(*last_hash, phash) <= config.phash_max_distance)
            .unwrap_or(true);

        if !(within_gap && visually_close) {
            flush(&mut current, &mut groups);
        }
        current.push((hash, phash));
        last_camera = Some(camera);
        last_time = time;
    }
    flush(&mut current, &mut groups);

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
            phash_max_distance: 12,
        }
    }

    fn insert_image(
        conn: &Connection,
        hash: &str,
        camera: &str,
        time_ms: i64,
        phash: i64,
        sharpness: f64,
    ) {
        conn.execute(
            "INSERT INTO contents (hash, byte_size, kind, phash, camera_make, camera_model, sharpness) \
             VALUES (?1, 1, 'image', ?2, ?3, 'M', ?4)",
            params![hash, phash, camera, sharpness],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source) \
             VALUES (?1, '/a', ?2, 'image', ?3, ?4, 'metadata')",
            params![format!("/a/{hash}.jpg"), format!("{hash}.jpg"), hash, time_ms],
        )
        .unwrap();
    }

    #[test]
    fn spare_shots_within_the_gap_group_together() {
        let (_d, conn) = seeded();
        let t = 1_700_000_000_000i64;
        insert_image(&conn, "s1", "Ricoh", t, 0b0000, 10.0);
        insert_image(&conn, "s2", "Ricoh", t + 20_000, 0b0011, 30.0); // distance 2
        insert_image(&conn, "s3", "Ricoh", t + 45_000, 0b0111, 20.0); // distance 1 from s2
        // A later, unrelated shot far outside the gap.
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
        // Two visually close, then a visual jump within the time gap, then two
        // close again: two groups, never one.
        insert_image(&conn, "c1", "Ricoh", t, 0x0000_0000_0000_00FF, 1.0);
        insert_image(&conn, "c2", "Ricoh", t + 10_000, 0x0000_0000_0000_00FE, 1.0);
        insert_image(&conn, "d1", "Ricoh", t + 20_000, 0x7FFF_FFFF_FFFF_0000, 1.0);
        insert_image(&conn, "d2", "Ricoh", t + 30_000, 0x7FFF_FFFF_FFFE_0000, 1.0);

        let stats = rebuild_groups(&conn, &config()).unwrap();
        assert_eq!(stats.groups, 2);
        assert_eq!(stats.grouped_items, 4);
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

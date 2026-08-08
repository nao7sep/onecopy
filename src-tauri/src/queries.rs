//! Read-model queries for the UI. The unit everywhere is the LOGICAL file:
//! hashed rows collapse by content hash (a logical item's display time is the
//! earliest resolved time among its copies), and unhashed rows (unique-size
//! other-files, which by construction have no duplicates) each stand alone.
//! Companions never appear — they ride with their primary.
//!
//! Month bucketing happens in Rust under the given display timezone, not in
//! SQL's UTC strftime: for a JST user, photos taken before 09:00 on the 1st
//! belong to the new month, and SQL's UTC month would misfile them.

use std::collections::HashMap;

use chrono::TimeZone;
use chrono_tz::Tz;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MonthSection {
    /// `"2016-03"`, or `"undated"` for the trailing section.
    pub month: String,
    pub count: u64,
}

#[derive(Serialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SectionCounts {
    pub images: Vec<MonthSection>,
    pub videos: Vec<MonthSection>,
    pub others: Vec<MonthSection>,
}

/// Logical items per kind per month (oldest month first, Undated last).
pub fn section_counts(conn: &Connection, display_tz: Tz) -> Result<SectionCounts, String> {
    // Hashed logical items: one row per content hash, earliest resolved time.
    let mut stmt = conn
        .prepare(
            "SELECT c.kind, MIN(p.resolved_utc_ms), \
             SUM(CASE WHEN p.resolved_source = 'undated' THEN 0 ELSE 1 END) \
             FROM contents c JOIN paths p ON p.content_hash = c.hash \
             WHERE p.missing = 0 AND p.companion_of IS NULL \
             GROUP BY c.hash",
        )
        .map_err(|e| e.to_string())?;
    let hashed: Vec<(String, Option<i64>, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    // Unhashed logical items: unique-size other-files, one row each.
    let mut stmt = conn
        .prepare(
            "SELECT kind, resolved_utc_ms FROM paths \
             WHERE missing = 0 AND companion_of IS NULL AND content_hash IS NULL \
               AND kind NOT IN ('image', 'video')",
        )
        .map_err(|e| e.to_string())?;
    let unhashed: Vec<(String, Option<i64>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let mut images: HashMap<String, u64> = HashMap::new();
    let mut videos: HashMap<String, u64> = HashMap::new();
    let mut others: HashMap<String, u64> = HashMap::new();

    let mut bucket = |kind: &str, month: String| {
        let map = match kind {
            "image" => &mut images,
            "video" => &mut videos,
            _ => &mut others,
        };
        *map.entry(month).or_insert(0) += 1;
    };

    for (kind, min_ms, resolved_count) in hashed {
        let month = match min_ms {
            Some(ms) if resolved_count > 0 => month_key(ms, display_tz),
            _ => "undated".to_string(),
        };
        bucket(&kind, month);
    }
    for (kind, ms) in unhashed {
        let month = match ms {
            Some(ms) => month_key(ms, display_tz),
            None => "undated".to_string(),
        };
        bucket(&kind, month);
    }

    Ok(SectionCounts {
        images: sorted_sections(images),
        videos: sorted_sections(videos),
        others: sorted_sections(others),
    })
}

fn month_key(unix_ms: i64, tz: Tz) -> String {
    use chrono::Datelike;
    match tz.timestamp_millis_opt(unix_ms) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            format!("{:04}-{:02}", dt.year(), dt.month())
        }
        chrono::LocalResult::None => "undated".to_string(),
    }
}

/// Oldest month first; Undated last.
fn sorted_sections(map: HashMap<String, u64>) -> Vec<MonthSection> {
    let mut sections: Vec<MonthSection> = map
        .into_iter()
        .map(|(month, count)| MonthSection { month, count })
        .collect();
    sections.sort_by(|a, b| match (a.month.as_str(), b.month.as_str()) {
        ("undated", "undated") => std::cmp::Ordering::Equal,
        ("undated", _) => std::cmp::Ordering::Greater,
        (_, "undated") => std::cmp::Ordering::Less,
        (a, b) => a.cmp(b),
    });
    sections
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_store;

    fn seeded() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-queries-")
            .tempdir()
            .unwrap();
        let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        (dir, conn)
    }

    fn utc_ms(y: i32, mo: u32, d: u32, h: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn copies_collapse_to_one_logical_item_with_the_earliest_time() {
        let (_d, conn) = seeded();
        conn.execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 1, 'image');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/a/x.jpg', '/a', 'x.jpg', 'image', 'h1', {t1}, 'metadata');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/b/x.jpg', '/b', 'x.jpg', 'image', 'h1', {t2}, 'filesystem');",
            t1 = utc_ms(2016, 3, 5, 3),
            t2 = utc_ms(2020, 1, 1, 0),
        ))
        .unwrap();

        let counts = section_counts(&conn, chrono_tz::UTC).unwrap();
        assert_eq!(
            counts.images,
            vec![MonthSection {
                month: "2016-03".into(),
                count: 1
            }]
        );
    }

    #[test]
    fn display_timezone_decides_the_month_boundary() {
        let (_d, conn) = seeded();
        // 2016-03-31T22:00:00Z is already April 1st in JST (+09:00).
        conn.execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 1, 'image');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/a/y.jpg', '/a', 'y.jpg', 'image', 'h1', {t}, 'metadata');",
            t = utc_ms(2016, 3, 31, 22),
        ))
        .unwrap();

        let utc = section_counts(&conn, chrono_tz::UTC).unwrap();
        assert_eq!(utc.images[0].month, "2016-03");
        let jst = section_counts(&conn, chrono_tz::Asia::Tokyo).unwrap();
        assert_eq!(jst.images[0].month, "2016-04");
    }

    #[test]
    fn unhashed_other_files_count_individually_and_companions_never_appear() {
        let (_d, conn) = seeded();
        conn.execute_batch(&format!(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/doc.pdf', '/a', 'doc.pdf', 'other', {t}, 'filesystem');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/undatable.bin', '/a', 'undatable.bin', 'other', NULL, 'undated');
             INSERT INTO contents (hash, byte_size, kind) VALUES ('v1', 1, 'video');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/a/clip.mp4', '/a', 'clip.mp4', 'video', 'v1', {t}, 'metadata');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, companion_of, resolved_utc_ms, resolved_source)
               VALUES ('/a/clip.thm', '/a', 'clip.thm', 'companion', 3, {t}, 'filesystem');",
            t = utc_ms(2019, 7, 10, 12),
        ))
        .unwrap();

        let counts = section_counts(&conn, chrono_tz::UTC).unwrap();
        assert_eq!(counts.videos, vec![MonthSection { month: "2019-07".into(), count: 1 }]);
        assert_eq!(
            counts.others,
            vec![
                MonthSection { month: "2019-07".into(), count: 1 },
                MonthSection { month: "undated".into(), count: 1 },
            ]
        );
        assert!(counts.images.is_empty());
    }

    #[test]
    fn undated_sorts_last_and_months_sort_oldest_first() {
        let (_d, conn) = seeded();
        conn.execute_batch(&format!(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/new.bin', '/a', 'new.bin', 'other', {new}, 'filesystem');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/old.bin', '/a', 'old.bin', 'other', {old}, 'filesystem');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/none.bin', '/a', 'none.bin', 'other', NULL, 'undated');",
            new = utc_ms(2024, 12, 1, 0),
            old = utc_ms(2009, 1, 1, 0),
        ))
        .unwrap();

        let counts = section_counts(&conn, chrono_tz::UTC).unwrap();
        let months: Vec<&str> = counts.others.iter().map(|s| s.month.as_str()).collect();
        assert_eq!(months, vec!["2009-01", "2024-12", "undated"]);
    }
}

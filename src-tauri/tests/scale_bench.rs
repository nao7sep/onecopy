// Scale measurements against the authorized temp corpus (up to ~100 clones of
// company/assets — the standing authorization; the corpus is DELETED after).
// `#[ignore]`d: these are measurements to run deliberately, not gates.
//
// Run:  ONECOPY_BENCH_CORPUS=/path/to/corpus cargo test --test scale_bench -- --ignored --nocapture

use std::time::Instant;

use onecopy_lib::index_store;
use onecopy_lib::queries;
use onecopy_lib::scanner;
use onecopy_lib::{derived_work, face, video};
use onecopy_lib::similarity::{rebuild_groups, SimilarityConfig};
use rusqlite::params;

#[test]
#[ignore]
fn six_item_section_in_a_million_row_index() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-bench-sections-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "BEGIN;
         INSERT INTO contents (hash, byte_size, kind) VALUES ('benchmark-anchor', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing)
           VALUES ('/anchor', '/', 'anchor', 'image', 'benchmark-anchor', 1);
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 999993
         )
         INSERT INTO contents (hash, byte_size, kind)
           SELECT printf('background-%07d', n), 100, 'image' FROM seq;
         INSERT INTO logical_contents
           (content_hash, kind, resolved_utc_ms, representative_path_id,
            live_copy_count, names_differ)
           SELECT hash, 'image', 1735689600000, 1, 1, 0 FROM contents
           WHERE hash LIKE 'background-%';
         COMMIT;",
    )
    .unwrap();
    for n in 0..6 {
        let hash = format!("target-{n}");
        conn.execute(
            "INSERT INTO contents (hash, byte_size, kind) VALUES (?1, 100, 'image')",
            [&hash],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, \
             resolved_utc_ms, resolved_source) VALUES (?1, '/target', ?2, 'image', ?3, \
             1767225600000, 'metadata')",
            params![format!("/target/{n}.jpg"), format!("{n}.jpg"), hash],
        )
        .unwrap();
    }

    let started = Instant::now();
    let items = queries::section_items(&conn, "image", "2026-01", chrono_tz::UTC).unwrap();
    eprintln!(
        "opened six items among one million logical rows in {:?}",
        started.elapsed()
    );
    assert_eq!(items.len(), 6);
}

#[test]
#[ignore]
fn capped_candidate_traversals_advance_once_across_one_million_pending_items() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-bench-derived-pages-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "BEGIN;
         INSERT INTO paths
           (id, abs_path, dir_path, file_name, kind, missing)
         VALUES (1, '/representative', '/', 'representative', 'other', 0);
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 999999
         )
         INSERT INTO contents
           (hash, byte_size, kind, duration_ms, derived_at_utc)
         SELECT printf('pending-%07d', n), 1,
                CASE WHEN n % 2 = 0 THEN 'video' ELSE 'image' END,
                CASE WHEN n % 2 = 0 THEN 30000 ELSE NULL END,
                'ready'
         FROM seq;
         INSERT INTO logical_contents
           (content_hash, kind, resolved_utc_ms, representative_path_id,
            live_copy_count, names_differ)
         SELECT hash, kind, CAST(substr(hash, 9) AS INTEGER), 1, 1, 0
         FROM contents;
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 999999
         )
         INSERT INTO paths
           (id, abs_path, dir_path, file_name, kind, indexed_at_utc, missing)
         SELECT n + 2, printf('/repair-%07d', n), '/', printf('repair-%07d', n),
                CASE WHEN n % 2 = 0 THEN 'video' ELSE 'image' END,
                'ready', 0
         FROM seq;
         COMMIT;",
    )
    .unwrap();

    let mut strip_after = None;
    let mut strip_total = 0usize;
    loop {
        let rows = video::strip_candidates(
            &conn,
            strip_after.as_deref(),
            video::STRIP_CANDIDATE_PAGE_SIZE,
        )
        .unwrap();
        if rows.is_empty() {
            break;
        }
        strip_total += rows.len();
        strip_after = rows.last().map(|row| row.0.clone());
    }

    let mut face_after = None;
    let mut face_total = 0usize;
    loop {
        let rows = face::face_candidates(
            &conn,
            face_after.as_deref(),
            face::FACE_CANDIDATE_PAGE_SIZE,
        )
        .unwrap();
        if rows.is_empty() {
            break;
        }
        face_total += rows.len();
        face_after = rows.last().map(|row| row.0.clone());
    }

    let mut transcript_after = None;
    let mut transcript_total = 0usize;
    loop {
        let rows = derived_work::transcript_candidates(
            &conn,
            transcript_after.as_deref(),
            derived_work::TRANSCRIPT_CANDIDATE_PAGE_SIZE,
        )
        .unwrap();
        if rows.is_empty() {
            break;
        }
        transcript_total += rows.len();
        transcript_after = rows.last().map(|row| row.0.clone());
    }

    let mut repair_after = 0i64;
    let mut repair_total = 0usize;
    loop {
        let rows = scanner::live_photo_repair_candidates(
            &conn,
            repair_after,
            scanner::LIVE_PHOTO_REPAIR_PAGE_SIZE,
        )
        .unwrap();
        if rows.is_empty() {
            break;
        }
        repair_total += rows.len();
        repair_after = rows.last().unwrap().0;
    }

    assert_eq!(strip_total, 500_000);
    assert_eq!(face_total, 500_000);
    assert_eq!(transcript_total, 500_000);
    assert_eq!(repair_total, 1_000_000);
}

#[test]
#[ignore]
fn scan_wall_clock_against_the_corpus() {
    let corpus = std::env::var("ONECOPY_BENCH_CORPUS").expect("set ONECOPY_BENCH_CORPUS");
    let home = tempfile::Builder::new()
        .prefix("onecopy-bench-home-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&home.path().join("index.sqlite3")).unwrap();

    let config = serde_json::json!({ "sourceDirs": [corpus] });
    let settings = scanner::settings_from_config(Some(&config), home.path(), 0);

    let started = Instant::now();
    let summary = scanner::run_full_scan(&conn, &settings, &|phase, detail| {
        eprintln!("[{:>7.1?}] {phase}: {detail}", started.elapsed());
    })
    .unwrap();
    eprintln!("TOTAL: {:?}  summary: {summary:?}", started.elapsed());
}

/// The Phase 15 question, answered synthetically: the rebuild is quadratic
/// WITHIN a month bucket, so the biggest bucket is the whole cost. Rows are
/// synthetic (integer work over in-memory candidates — no files are read by
/// design), which is exactly what makes the measurement honest for the
/// quadratic question and cheap to run at any size.
#[test]
#[ignore]
fn similarity_rebuild_cost_by_bucket_size() {
    for n in [10_000u64, 30_000, 90_000] {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-bench-sim-")
            .tempdir()
            .unwrap();
        let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        // One month bucket: same resolved month for every row; phashes vary
        // pseudo-randomly (splitmix64) so pairing stays realistic-sparse.
        let base_ms = 1_700_000_000_000i64;
        let tx_started = Instant::now();
        conn.execute_batch("BEGIN").unwrap();
        let mut x = 0x9E3779B97F4A7C15u64;
        for i in 0..n {
            x ^= x >> 30;
            x = x.wrapping_mul(0xBF58476D1CE4E5B9);
            x ^= x >> 27;
            let phash = (x & 0x7FFF_FFFF_FFFF_FFFF) as i64;
            conn.execute(
                "INSERT INTO contents (hash, byte_size, kind, phash, sharpness) \
                 VALUES (?1, 100, 'image', ?2, 1.0)",
                params![format!("h{i}"), phash],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, \
                 mtime_ms, content_hash, resolved_utc_ms, resolved_source, date_only, missing, companion_of) \
                 VALUES (?1, '/b', ?2, ?3, 'jpg', 'image', 100, 0, ?4, ?5, 'metadata', 0, 0, NULL)",
                params![
                    format!("/b/{i}.jpg"),
                    format!("{i}.jpg"),
                    format!("{i}"),
                    format!("h{i}"),
                    base_ms + (i as i64 % 3600) * 1000
                ],
            )
            .unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
        eprintln!("seeded {n} rows in {:?}", tx_started.elapsed());

        let config = SimilarityConfig {
            phash_max_distance_burst: 10,
            max_gap_seconds: 90,
            diameter_multiplier: 2,
            phash_max_distance: 4,
            };
        let started = Instant::now();
        let stats = rebuild_groups(&conn, &config).unwrap();
        eprintln!("rebuild at n={n}: {:?}  ({stats:?})", started.elapsed());
    }
}

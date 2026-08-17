// Scale measurements against the authorized temp corpus (up to ~100 clones of
// company/assets — the standing authorization; the corpus is DELETED after).
// `#[ignore]`d: these are measurements to run deliberately, not gates.
//
// Run:  ONECOPY_BENCH_CORPUS=/path/to/corpus cargo test --test scale_bench -- --ignored --nocapture

use std::time::Instant;

use onecopy_lib::index_store;
use onecopy_lib::scanner;
use onecopy_lib::similarity::{rebuild_groups, SimilarityConfig};
use rusqlite::params;

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

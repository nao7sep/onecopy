// Scale measurements against the authorized temp corpus (up to ~100 clones of
// company/assets — the standing authorization; the corpus is DELETED after).
// `#[ignore]`d: these are measurements to run deliberately, not gates.
//
// Run:  ONECOPY_BENCH_CORPUS=/path/to/corpus cargo test --test scale_bench -- --ignored --nocapture

use std::time::Instant;

use onecopy_lib::background_work;
use onecopy_lib::derived_runtime::{self, RuntimeConditions};
use onecopy_lib::derived_state;
use onecopy_lib::index_store;
use onecopy_lib::queries;
use onecopy_lib::scanner;
use onecopy_lib::similarity::cluster_by_appearance;
use onecopy_lib::viewer_sequence;
use rusqlite::params;

fn item_projection() -> queries::ItemProjectionContext {
    queries::ItemProjectionContext {
        capabilities: derived_state::WorkCapabilities {
            ffmpeg: true,
            video_snapshots_enabled: true,
            similarity_enabled: true,
            face_enabled: false,
            face_models: false,
            transcription_model: false,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
        },
        similarity_dirty: false,
    }
}

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
           (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
            live_copy_count)
           SELECT hash, 'image', 'dated', 1735689600000, 1, 1 FROM contents
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
    let items = queries::section_window(
        &conn,
        "image",
        "2026-01",
        chrono_tz::UTC,
        queries::SectionSort {
            order: queries::SectionSortOrder::Time,
            desc: false,
        },
        0,
        6,
        item_projection(),
    )
    .unwrap()
    .items;
    eprintln!(
        "opened six items among one million logical rows in {:?}",
        started.elapsed()
    );
    assert_eq!(items.len(), 6);
}

#[test]
#[ignore]
fn bounded_window_of_one_million_active_undated_items() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-bench-active-section-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "BEGIN;
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 999999
         )
         INSERT INTO paths
           (id, abs_path, dir_path, file_name, stem, ext, kind, missing)
         SELECT n + 1, printf('/undated/%07d.jpg', n), '/undated',
                printf('%07d.jpg', n), printf('%07d', n), 'jpg', 'image', 1
         FROM seq;
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 999999
         )
         INSERT INTO contents (hash, byte_size, kind, width, height)
         SELECT printf('active-%07d', n), n + 1, 'image', 4000, 3000 FROM seq;
         INSERT INTO logical_contents
           (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
            live_copy_count)
         SELECT hash, 'image', 'undated', NULL,
                CAST(substr(hash, 8) AS INTEGER) + 1, 1
         FROM contents;
         COMMIT;",
    )
    .unwrap();

    let started = Instant::now();
    let window = queries::section_window(
        &conn,
        "image",
        "undated",
        chrono_tz::UTC,
        queries::SectionSort {
            order: queries::SectionSortOrder::Name,
            desc: false,
        },
        500_000,
        256,
        item_projection(),
    )
    .unwrap();
    eprintln!(
        "opened 256 rows in a million-item active section in {:?}",
        started.elapsed()
    );
    assert_eq!(window.total, 1_000_000);
    assert_eq!(window.start, 500_000);
    assert_eq!(window.items.len(), 256);
    assert_eq!(window.items[0].file_name, "0500000.jpg");

    let sequence_started = Instant::now();
    let snapshot = viewer_sequence::start(
        dir.path(),
        &conn,
        "image",
        "undated",
        chrono_tz::UTC,
        queries::SectionSort {
            order: queries::SectionSortOrder::Name,
            desc: false,
        },
        vec![queries::PositionedSectionIdentity {
            hash: Some("active-0500000".to_string()),
            path_id: 500_001,
            index: 500_000,
        }],
        &queries::SectionIdentity {
            hash: Some("active-0500000".to_string()),
            path_id: 500_001,
        },
        item_projection(),
    )
    .unwrap();
    eprintln!(
        "froze a million-item viewer sequence in {:?}",
        sequence_started.elapsed()
    );
    assert_eq!(snapshot.length, 1_000_000);
    assert_eq!(snapshot.index, 500_000);
    viewer_sequence::close(Some(&snapshot.token)).unwrap();
}

#[test]
#[ignore]
fn section_counts_across_one_million_logical_items() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-bench-section-counts-")
        .tempdir()
        .unwrap();
    let db = dir.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch(
        "BEGIN;
         INSERT INTO paths
           (id, abs_path, dir_path, file_name, kind, missing)
         VALUES (1, '/representative', '/', 'representative', 'other', 1);
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 999999
         )
         INSERT INTO contents (hash, byte_size, kind)
         SELECT printf('count-%07d', n), 1,
                CASE n % 3 WHEN 0 THEN 'image' WHEN 1 THEN 'video' ELSE 'other' END
         FROM seq;
         INSERT INTO logical_contents
           (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
            live_copy_count)
         SELECT hash, kind, 'dated',
                1262304000000 + (CAST(substr(hash, 7) AS INTEGER) % 5844) * 86400000,
                1, 1
         FROM contents;
         COMMIT;",
    )
    .unwrap();

    let first = Instant::now();
    let counts = queries::cached_section_counts(&db, chrono_tz::Asia::Tokyo).unwrap();
    eprintln!("first million-row section count in {:?}", first.elapsed());
    let total: u64 = counts
        .images
        .iter()
        .chain(&counts.videos)
        .chain(&counts.others)
        .map(|section| section.count)
        .sum();
    assert_eq!(total, 1_000_000);

    let repeated = Instant::now();
    let repeated_counts = queries::cached_section_counts(&db, chrono_tz::Asia::Tokyo).unwrap();
    eprintln!(
        "repeated million-row section count in {:?}",
        repeated.elapsed()
    );
    assert_eq!(repeated_counts, counts);
}

#[test]
#[ignore]
fn first_issues_page_among_one_million_current_diagnostics() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-bench-issues-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "BEGIN;
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 999999
         )
         INSERT INTO issues
           (path, kind, message, first_seen_utc, last_seen_utc)
         SELECT printf('/issue-%07d', n), 'read-error', 'synthetic',
                printf('2026-01-%07d', 999999 - n),
                printf('2026-02-%07d', n)
         FROM seq;
         COMMIT;",
    )
    .unwrap();

    let started = Instant::now();
    let (total, rows) = queries::issues(&conn, 500).unwrap();
    eprintln!(
        "opened 500 of one million current Issues in {:?}",
        started.elapsed()
    );
    assert_eq!(total, 1_000_000);
    assert_eq!(rows.len(), 500);
}

#[test]
#[ignore]
fn section_repair_collects_directories_without_materializing_one_million_items() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-bench-section-dirs-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "BEGIN;
         INSERT INTO paths
           (id, abs_path, dir_path, file_name, kind, missing)
         VALUES (1, '/representative', '/', 'representative', 'other', 1);
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 999999
         )
         INSERT INTO contents (hash, byte_size, kind)
         SELECT printf('repair-section-%07d', n), 1, 'image' FROM seq;
         INSERT INTO logical_contents
           (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
            live_copy_count)
         SELECT hash, 'image', 'dated', 1735689600000, 1, 1 FROM contents;
         COMMIT;",
    )
    .unwrap();

    let started = Instant::now();
    let dirs = queries::section_dirs(&conn, "image", "2025-01", chrono_tz::UTC).unwrap();
    eprintln!(
        "collected section repair directories across one million items in {:?}",
        started.elapsed()
    );
    assert!(dirs.is_empty());
}

#[test]
#[ignore]
fn background_work_snapshot_across_one_million_live_items() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-bench-background-snapshot-")
        .tempdir()
        .unwrap();
    let conn =
        index_store::open(&dir.path().join(onecopy_lib::storage::INDEX_DB_FILE_NAME)).unwrap();
    conn.execute_batch(
        "BEGIN;
         INSERT INTO paths
           (id, abs_path, dir_path, file_name, kind, missing)
         VALUES (1, '/representative', '/', 'representative', 'other', 1);
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 999999
         )
         INSERT INTO contents
           (hash, byte_size, kind, duration_ms, derived_at_utc, derived_version)
         SELECT printf('background-work-%07d', n), 1,
                CASE n % 4 WHEN 0 THEN 'image' WHEN 1 THEN 'image' ELSE 'video' END,
                CASE WHEN n % 4 >= 2 THEN 60000 ELSE NULL END,
                CASE WHEN n % 4 IN (1, 3) THEN '2026-01-01T00:00:00.000Z' ELSE NULL END,
                CASE WHEN n % 4 IN (1, 3) THEN 3 ELSE 0 END
         FROM seq;
         INSERT INTO logical_contents
           (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
            live_copy_count)
         SELECT hash, kind, 'pending', NULL, 1, 1 FROM contents;
         COMMIT;",
    )
    .unwrap();
    drop(conn);

    let runtime = derived_runtime::snapshot(RuntimeConditions {
        busy: false,
        similarity_dirty: false,
    })
    .unwrap();
    let started = Instant::now();
    let snapshot = background_work::snapshot(
        dir.path(),
        runtime,
        derived_state::WorkCapabilities {
            ffmpeg: true,
            video_snapshots_enabled: true,
            similarity_enabled: true,
            face_enabled: true,
            face_models: true,
            transcription_model: true,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
        },
    )
    .unwrap();
    eprintln!(
        "projected Background work across one million items in {:?}",
        started.elapsed()
    );
    let value = serde_json::to_value(snapshot).unwrap();
    let classes = value["classes"].as_array().unwrap();
    assert_eq!(classes.len(), 5);
    assert_eq!(classes[0]["queued"], 500_000);
    assert_eq!(classes[1]["queued"], 250_000);
    assert_eq!(classes[3]["queued"], 250_000);
    assert_eq!(classes[4]["queued"], 500_000);
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
           (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
            live_copy_count)
         SELECT hash, kind, 'dated', CAST(substr(hash, 9) AS INTEGER), 1, 1
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
        let rows = derived_state::strip_candidates(
            &conn,
            strip_after.as_deref(),
            derived_state::SNAPSHOT_CANDIDATE_PAGE_SIZE,
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
        let rows = derived_state::face_candidates(
            &conn,
            face_after.as_deref(),
            derived_state::FACE_CANDIDATE_PAGE_SIZE,
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
        let rows = derived_state::transcript_candidates(
            &conn,
            "video",
            transcript_after.as_deref(),
            derived_state::TRANSCRIPT_CANDIDATE_PAGE_SIZE,
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
fn scoped_pairing_stays_inside_one_directory_among_one_million_duplicate_tree_paths() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-bench-pair-scope-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let seeded = Instant::now();
    conn.execute_batch(
        "BEGIN;
         WITH RECURSIVE seq(n) AS (
           VALUES(0) UNION ALL SELECT n + 1 FROM seq WHERE n < 499999
         ), kind(slot, name, ext) AS (
           VALUES(0, 'still', 'jpg'), (1, 'motion', 'mov')
         )
         INSERT INTO paths
           (id, abs_path, dir_path, file_name, stem, ext, kind, missing)
         SELECT n * 2 + slot + 1,
                printf('/backup-%06d/%s.%s', n, name, ext),
                printf('/backup-%06d', n),
                printf('%s.%s', name, ext), name, ext,
                CASE slot WHEN 0 THEN 'image' ELSE 'video' END, 0
         FROM seq CROSS JOIN kind;
         INSERT INTO evidence (path_id, source, raw)
           SELECT id, 'live-photo-identifier', 'shared-across-every-backup'
           FROM paths;
         COMMIT;",
    )
    .unwrap();
    eprintln!(
        "seeded one million paths and evidence rows in {:?}",
        seeded.elapsed()
    );

    let target = vec!["/backup-499999".to_string()];
    let started = Instant::now();
    let stats = scanner::pair_companions_in_dirs(&conn, true, &target).unwrap();
    eprintln!("scoped one-directory pairing in {:?}", started.elapsed());
    assert_eq!(stats.paired, 1);
    let links: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM paths WHERE companion_of IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(links, 1, "the other 499,999 cohorts remain untouched");

    let enabled = Instant::now();
    assert_eq!(
        scanner::pair_companions(&conn, true).unwrap().paired,
        500_000
    );
    eprintln!(
        "globally enabled 500,000 cohorts in {:?}",
        enabled.elapsed()
    );
    let disabled = Instant::now();
    assert_eq!(scanner::pair_companions(&conn, false).unwrap().paired, 0);
    eprintln!("globally disabled pairing in {:?}", disabled.elapsed());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM paths WHERE companion_of IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    let reenabled = Instant::now();
    assert_eq!(
        scanner::pair_companions(&conn, true).unwrap().paired,
        500_000
    );
    eprintln!(
        "globally re-enabled 500,000 cohorts in {:?}",
        reenabled.elapsed()
    );
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
    let last_phase = std::cell::Cell::new(None);
    let summary = scanner::run_full_scan(&conn, &settings, &|progress| {
        if last_phase.get() != Some(progress.phase) || progress.done == progress.total {
            eprintln!(
                "[{:>7.1?}] {:?}: {}/{}",
                started.elapsed(),
                progress.phase,
                progress.done,
                progress.total
            );
            last_phase.set(Some(progress.phase));
        }
    })
    .unwrap();
    eprintln!("TOTAL: {:?}  summary: {summary:?}", started.elapsed());
}

/// Exact candidate generation inside one deliberately large month bucket.
/// Synthetic integer work isolates the strict Hamming bands and relaxed
/// capture-time window from SQLite setup and file I/O.
#[test]
#[ignore]
fn similarity_candidates_by_bucket_size() {
    for n in [10_000usize, 100_000, 300_000] {
        let mut phashes = Vec::with_capacity(n);
        let mut times = Vec::with_capacity(n);
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for i in 0..n {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut hash = state;
            hash = (hash ^ (hash >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            hash = (hash ^ (hash >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            phashes.push((hash ^ (hash >> 31)) as i64);
            times.push(Some(1_700_000_000_000 + (i % 86_400) as i64 * 30_000));
        }
        let started = Instant::now();
        let clusters = cluster_by_appearance(&phashes, &times, 4, 10, 90, 2).unwrap();
        eprintln!(
            "exact similarity candidates at n={n}: {:?} ({} clusters)",
            started.elapsed(),
            clusters.len()
        );
        assert_eq!(clusters.iter().map(Vec::len).sum::<usize>(), n);
    }
}

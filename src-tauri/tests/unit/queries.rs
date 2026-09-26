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

fn projection() -> ItemProjectionContext {
    ItemProjectionContext {
        capabilities: crate::derived_state::WorkCapabilities {
            ffmpeg: true,
            video_snapshots_enabled: true,
            similarity_enabled: true,
            face_enabled: false,
            face_models: false,
            transcription_model: false,
            transcription_acceleration: true,
            face_acceleration: true,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
        },
    }
}

fn section_items(
    conn: &Connection,
    kind: &str,
    month: &str,
    display_tz: Tz,
    projection: ItemProjectionContext,
) -> Result<Vec<SectionItem>, String> {
    section_window(
        conn,
        kind,
        month,
        display_tz,
        SectionSort {
            order: SectionSortOrder::Time,
            desc: false,
        },
        0,
        MAX_SECTION_WINDOW_ITEMS,
        projection,
    )
    .map(|window| window.items)
}

fn utc_ms(y: i32, mo: u32, d: u32, h: u32) -> i64 {
    chrono::NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

// EXCEPTION (tests-folder conventions): the query-plan assertion must use
// the private SQL builder that production executes; copying the SQL into
// an integration test would let the test and implementation drift apart.
#[test]
fn a_month_section_seeks_the_logical_section_index() {
    let (_dir, conn) = seeded();
    let candidates = section_candidates_sql(false, true);
    let sql = format!(
        "EXPLAIN QUERY PLAN WITH candidates AS ({candidates}) \
         SELECT hash, path_id FROM candidates ORDER BY {} LIMIT 512 OFFSET 0",
        section_order_sql(SectionSort {
            order: SectionSortOrder::Time,
            desc: false,
        })
    );
    let mut stmt = conn.prepare(&sql).unwrap();
    let details: Vec<String> = stmt
        .query_map(
            rusqlite::params!["image", utc_ms(2026, 1, 1, 0), utc_ms(2026, 2, 1, 0)],
            |row| row.get(3),
        )
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        details
            .iter()
            .any(|line| line.contains("idx_logical_contents_section")),
        "section query lost its indexed month seek: {details:?}"
    );
    assert!(
        details.iter().all(|line| !line.starts_with("SCAN p")),
        "section query regressed to a whole paths scan: {details:?}"
    );
}

#[test]
fn section_repair_seeks_directly_to_directory_facts() {
    let (_dir, conn) = seeded();
    let plans = [
        (
            section_dirs_sql(true),
            vec![
                rusqlite::types::Value::Text("image".to_string()),
                0.into(),
                1.into(),
            ],
            vec!["idx_logical_contents_section", "idx_paths_content_hash"],
        ),
        (
            unhashed_other_section_dirs_sql(true),
            vec![0.into(), 1.into()],
            vec!["idx_paths_unhashed_other_section"],
        ),
    ];
    for (sql, params, expected_indexes) in plans {
        let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let details: Vec<String> = statement
            .query_map(rusqlite::params_from_iter(params), |row| row.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for expected in expected_indexes {
            assert!(
                details.iter().any(|line| line.contains(expected)),
                "section repair lost {expected}: {details:?}"
            );
        }
        assert!(
            details.iter().all(|line| !line.starts_with("SCAN p")),
            "section repair regressed to a whole paths scan: {details:?}"
        );
    }
}

#[test]
fn sidebar_count_queries_seek_month_and_edge_indexes() {
    let (_dir, conn) = seeded();
    let plans = [
        (
            LOGICAL_MONTH_COUNT_SQL.to_string(),
            vec![
                rusqlite::types::Value::Text("image".to_string()),
                0.into(),
                1.into(),
            ],
            "idx_logical_contents_section",
        ),
        (
            UNHASHED_OTHER_MONTH_COUNT_SQL.to_string(),
            vec![0.into(), 1.into()],
            "idx_paths_unhashed_other_section",
        ),
        (
            logical_edge_sql(false),
            vec![rusqlite::types::Value::Text("image".to_string())],
            "idx_logical_contents_section",
        ),
        (
            unhashed_other_edge_sql(false),
            Vec::new(),
            "idx_paths_unhashed_other_section",
        ),
    ];
    for (sql, params, expected_index) in plans {
        let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let details: Vec<String> = statement
            .query_map(rusqlite::params_from_iter(params), |row| row.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            details.iter().any(|line| line.contains(expected_index)),
            "sidebar count query lost {expected_index}: {details:?}"
        );
        assert!(
            details.iter().all(|line| !line.contains("USE TEMP B-TREE")),
            "sidebar count query introduced temporary sorting: {details:?}"
        );
    }
}

#[test]
fn issues_page_seeks_oldest_diagnostics_without_sorting() {
    let (_dir, conn) = seeded();
    let mut statement = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {ISSUES_PAGE_SQL}"))
        .unwrap();
    let details: Vec<String> = statement
        .query_map([500], |row| row.get(3))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        details
            .iter()
            .any(|line| line.contains("idx_issues_first_seen")),
        "Issues page lost its oldest-first index: {details:?}"
    );
    assert!(
        details.iter().all(|line| !line.contains("USE TEMP B-TREE")),
        "Issues page reintroduced whole-table sorting: {details:?}"
    );
}

#[test]
fn section_count_cache_tracks_sqlite_revision_and_timezone() {
    let (dir, writer) = seeded();
    writer
        .execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 1, 'image');
             INSERT INTO paths
               (abs_path, dir_path, file_name, kind, content_hash,
                resolved_utc_ms, resolved_source)
             VALUES ('/h1.jpg', '/', 'h1.jpg', 'image', 'h1', {boundary}, 'metadata');",
            boundary = utc_ms(2016, 3, 31, 22),
        ))
        .unwrap();
    let db = dir.path().join("index.sqlite3");
    let mut cache = SectionCountsCache::open(&db).unwrap();

    let (tokyo, first_recomputed) = cache.load(chrono_tz::Asia::Tokyo).unwrap();
    assert!(first_recomputed);
    assert_eq!(tokyo.images[0].month, "2016-04");
    let (_, repeated_recomputed) = cache.load(chrono_tz::Asia::Tokyo).unwrap();
    assert!(!repeated_recomputed);

    writer
        .execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h2', 1, 'image');
             INSERT INTO paths
               (abs_path, dir_path, file_name, kind, content_hash,
                resolved_utc_ms, resolved_source)
             VALUES ('/h2.jpg', '/', 'h2.jpg', 'image', 'h2', {earlier}, 'metadata');",
            earlier = utc_ms(2016, 3, 1, 0),
        ))
        .unwrap();
    let (updated, revision_recomputed) = cache.load(chrono_tz::Asia::Tokyo).unwrap();
    assert!(revision_recomputed);
    assert_eq!(
        updated.images.iter().map(|month| month.count).sum::<u64>(),
        2
    );

    let (utc, timezone_recomputed) = cache.load(chrono_tz::UTC).unwrap();
    assert!(timezone_recomputed);
    assert_eq!(
        utc.images,
        vec![MonthSection {
            month: "2016-03".into(),
            count: 2
        }]
    );
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
fn section_items_filters_by_month_and_reports_copies_and_thumbs() {
    let (_d, conn) = seeded();
    conn.execute_batch(&format!(
        "INSERT INTO contents
           (hash, byte_size, kind, width, height, derived_at_utc, derived_version)
           VALUES
           ('march', 1, 'image', 4000, 3000, '2026-08-08T00:00:00.000Z', {version});
         INSERT INTO contents (hash, byte_size, kind) VALUES ('april', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/a/m.jpg', '/a', 'm.jpg', 'image', 'march', {mar}, 'metadata');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/b/m.jpg', '/b', 'm.jpg', 'image', 'march', {mar2}, 'filesystem');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/a/a.jpg', '/a', 'a.jpg', 'image', 'april', {apr}, 'metadata');",
        mar = utc_ms(2016, 3, 5, 3),
        mar2 = utc_ms(2016, 3, 6, 3),
        apr = utc_ms(2016, 4, 2, 3),
        version = crate::derived_state::DERIVE_VERSION,
    ))
    .unwrap();

    let march = section_items(&conn, "image", "2016-03", chrono_tz::UTC, projection()).unwrap();
    assert_eq!(march.len(), 1);
    assert_eq!(march[0].hash.as_deref(), Some("march"));
    assert_eq!(march[0].copy_count, 2);
    assert!(march[0].has_thumb);
    assert_eq!(march[0].resolved_utc_ms, Some(utc_ms(2016, 3, 5, 3)));

    let april = section_items(&conn, "image", "2016-04", chrono_tz::UTC, projection()).unwrap();
    assert_eq!(april.len(), 1);
    assert!(!april[0].has_thumb);

    assert!(section_items(&conn, "image", "2016-05", chrono_tz::UTC, projection())
        .unwrap()
        .is_empty());
}

#[test]
fn item_detail_lists_copies_and_companions() {
    let (_d, conn) = seeded();
    conn.execute_batch(&format!(
        "INSERT INTO contents (hash, byte_size, kind, width, height) VALUES ('h1', 42, 'image', 4000, 3000);
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/a/x.jpg', '/a', 'x.jpg', 'image', 'h1', {t}, 'metadata');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/b/x.jpg', '/b', 'x.jpg', 'image', 'h1', {t}, 'metadata');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, companion_of, resolved_utc_ms, resolved_source)
           VALUES ('/a/x.arw', '/a', 'x.arw', 'companion', 1, {t}, 'filesystem');",
        t = utc_ms(2016, 3, 5, 3),
    ))
    .unwrap();

    let detail = item_detail(&conn, Some("h1"), None).unwrap();
    assert_eq!(detail.file_name, "x.jpg");
    assert_eq!(detail.byte_size, Some(42));
    assert_eq!(detail.width, Some(4000));
    assert_eq!(detail.copy_paths, vec!["/a/x.jpg", "/b/x.jpg"]);
    assert_eq!(detail.companion_paths, vec!["/a/x.arw"]);
    assert_eq!(detail.resolved_source.as_deref(), Some("metadata"));
}

#[test]
fn item_detail_hides_windows_verbatim_prefixes() {
    let (_d, conn) = seeded();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 42, 'image');",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
         VALUES (?1, ?2, 'x.jpg', 'image', 'h1')",
        [r"\\?\C:\photos\x.jpg", r"\\?\C:\photos"],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, companion_of) \
         VALUES (?1, ?2, 'x.xmp', 'companion', 1)",
        [r"\\?\C:\photos\x.xmp", r"\\?\C:\photos"],
    )
    .unwrap();

    let detail = item_detail(&conn, Some("h1"), None).unwrap();
    assert_eq!(detail.copy_paths, vec![r"C:\photos\x.jpg"]);
    assert_eq!(detail.companion_paths, vec![r"C:\photos\x.xmp"]);
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

#[test]
fn get_issues_clamps_an_unbounded_client_limit() {
    let (_dir, conn) = seeded();
    for index in 0..(MAX_ISSUES_PAGE_SIZE as usize + 20) {
        crate::index_store::upsert_issue(
            &conn,
            Some(&format!("/a/{index}.jpg")),
            "preview-error",
            "decode failed",
        )
        .unwrap();
    }

    let (total, rows) = issues(&conn, u32::MAX).unwrap();
    assert_eq!(total, MAX_ISSUES_PAGE_SIZE as u64 + 20);
    assert_eq!(
        rows.len(),
        MAX_ISSUES_PAGE_SIZE as usize,
        "a client-supplied limit above the cap must still be served at the cap"
    );
}

fn seed_month_image(conn: &Connection, hash: &str, name: &str, t: i64) {
    conn.execute_batch(&format!(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('{hash}', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
           VALUES ('/a/{name}', '/a', '{name}', 'image', '{hash}', {t}, 'metadata');"
    ))
    .unwrap();
}

#[test]
fn section_order_is_sorted_once_per_index_revision_and_reused_across_callers() {
    let (_d, conn) = seeded();
    seed_month_image(&conn, "h1", "a.jpg", utc_ms(2016, 3, 1, 0));
    seed_month_image(&conn, "h2", "b.jpg", utc_ms(2016, 3, 2, 0));
    seed_month_image(&conn, "h3", "c.jpg", utc_ms(2016, 3, 3, 0));

    let time_desc = SectionSort {
        order: SectionSortOrder::Time,
        desc: true,
    };
    let bounds = month_bounds("2016-03", chrono_tz::UTC).unwrap();

    let order = |conn: &Connection| {
        let snapshot = SectionSnapshot::begin(conn).unwrap();
        ordered_section_identities(&snapshot, "image", "2016-03", bounds, time_desc).unwrap()
    };
    let first = order(&conn);
    assert_eq!(first.len(), 3);

    // Reading again with no committed change reuses the exact same ordering
    // (same Arc allocation): the section is sorted once per revision, not
    // once per caller (reconcile_section, section windows, range reads, and
    // family-context recovery all call through this same entry point).
    let second = order(&conn);
    assert!(
        Arc::ptr_eq(&first, &second),
        "an unchanged index revision must reuse the cached ordering rather than re-sorting"
    );

    // A committed write bumps the index revision, so the next read recomputes
    // instead of serving a stale ordering.
    seed_month_image(&conn, "h4", "d.jpg", utc_ms(2016, 3, 4, 0));
    let third = order(&conn);
    assert!(
        !Arc::ptr_eq(&first, &third),
        "a new committed write must invalidate the cached ordering"
    );
    assert_eq!(third.len(), 4);
}

#[test]
fn section_order_cache_does_not_serve_a_stale_order_after_this_same_connection_writes() {
    // Regression guard for a subtler trap than a cross-connection write:
    // `PRAGMA data_version` only advances for a *different* connection's
    // commit, so a cache keyed on it alone would miss this connection's own
    // write. Ordinary command dispatch never shares one connection across a
    // write and a later read (Phase 3 opens a fresh connection per command),
    // but the cache must still be correct if that ever changes.
    let (_d, conn) = seeded();
    seed_month_image(&conn, "h1", "a.jpg", utc_ms(2016, 3, 1, 0));
    let time_desc = SectionSort {
        order: SectionSortOrder::Time,
        desc: true,
    };
    let bounds = month_bounds("2016-03", chrono_tz::UTC).unwrap();

    let order = |conn: &Connection| {
        let snapshot = SectionSnapshot::begin(conn).unwrap();
        ordered_section_identities(&snapshot, "image", "2016-03", bounds, time_desc).unwrap()
    };
    let before = order(&conn);
    assert_eq!(before.len(), 1);

    conn.execute("UPDATE paths SET missing = 1 WHERE file_name = 'a.jpg'", [])
        .unwrap();

    let after = order(&conn);
    assert_eq!(
        after.len(),
        0,
        "this connection's own write must be visible on the next read"
    );
}

#[test]
fn section_order_follows_a_write_committed_by_another_connection() {
    // Production opens a fresh connection per command: one command reads the
    // section, another commits a Delete, and a third reads again. A fresh
    // connection's own `data_version` never changes, so the revision must come
    // from an observer that sees every other connection's commits.
    let (dir, writer) = seeded();
    let db_file = dir.path().join("index.sqlite3");
    seed_month_image(&writer, "h1", "a.jpg", utc_ms(2016, 3, 1, 0));
    seed_month_image(&writer, "h2", "b.jpg", utc_ms(2016, 3, 2, 0));
    let time_desc = SectionSort {
        order: SectionSortOrder::Time,
        desc: true,
    };
    let read = || {
        let reader = index_store::open(&db_file).unwrap();
        section_window(
            &reader,
            "image",
            "2016-03",
            chrono_tz::UTC,
            time_desc,
            0,
            10,
            projection(),
        )
        .unwrap()
    };

    assert_eq!(read().total, 2);
    writer
        .execute("UPDATE paths SET missing = 1 WHERE file_name = 'a.jpg'", [])
        .unwrap();
    let after_delete = read();
    assert_eq!(after_delete.total, 1, "a committed removal must leave the order");
    assert_eq!(after_delete.items[0].file_name, "b.jpg");

    seed_month_image(&writer, "h3", "c.jpg", utc_ms(2016, 3, 3, 0));
    assert_eq!(read().total, 2, "a committed discovery must enter the order");
}

#[test]
fn section_window_order_token_changes_exactly_when_the_order_does() {
    // Main keeps selection positions from its last reconcile and refreshes
    // only the displayed window while the order token is unchanged; any
    // insertion, removal or unrelated write must be told apart by the token.
    let (dir, writer) = seeded();
    let db_file = dir.path().join("index.sqlite3");
    seed_month_image(&writer, "h1", "a.jpg", utc_ms(2016, 3, 1, 2));
    seed_month_image(&writer, "h2", "b.jpg", utc_ms(2016, 3, 2, 0));
    let time_asc = SectionSort {
        order: SectionSortOrder::Time,
        desc: false,
    };
    let read = |start: u64| {
        let reader = index_store::open(&db_file).unwrap();
        section_window(&reader, "image", "2016-03", chrono_tz::UTC, time_asc, start, 1, projection())
            .unwrap()
    };

    let first = read(0);
    assert_eq!(read(1).order, first.order, "every window of one order shares its token");

    writer
        .execute("UPDATE contents SET width = 640 WHERE hash = 'h2'", [])
        .unwrap();
    assert_eq!(read(0).order, first.order, "a write that keeps the order keeps its token");

    seed_month_image(&writer, "h0", "z.jpg", utc_ms(2016, 3, 1, 1));
    let inserted = read(0);
    assert_ne!(inserted.order, first.order, "an item entering above must change the token");

    writer
        .execute("UPDATE paths SET missing = 1 WHERE file_name = 'z.jpg'", [])
        .unwrap();
    assert_ne!(read(0).order, inserted.order, "an item leaving must change the token");
}

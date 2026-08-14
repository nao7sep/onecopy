// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).
//
// queries.rs had no test file, yet it produces everything the user actually
// looks at: the grid's rows, the thumbnail flag, the copy count, the section
// directories a rescan walks, and the similar-group membership the whole
// comparison surface renders.

use chrono_tz::Tz;
use rusqlite::{params, Connection};
use onecopy_lib::index_store;
use onecopy_lib::preview;
use onecopy_lib::queries;

fn db() -> Connection {
    let dir = std::env::temp_dir().join(format!(
        "onecopy-queries-{}",
        std::process::id() as u64 + rand_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    index_store::open(&dir.join("index.sqlite3")).unwrap()
}

// A deterministic-enough suffix without pulling in a rng: the connection is
// per-test and the file is disposable.
fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::SeqCst)
}

/// One image content row with a live path in January 2026 UTC.
fn seed_image(conn: &Connection, hash: &str, derived_at: Option<&str>, name: &str) {
    conn.execute(
        "INSERT INTO contents (hash, kind, byte_size, width, height, derived_at_utc) \
         VALUES (?1, 'image', 100, 640, 480, ?2)",
        params![hash, derived_at],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms, \
         content_hash, resolved_utc_ms, resolved_source, date_only, missing, companion_of) \
         VALUES (?1, '/root', ?2, ?3, 'jpg', 'image', 100, 0, ?4, ?5, 'metadata', 0, 0, NULL)",
        params![
            format!("/root/{name}"),
            name,
            name.trim_end_matches(".jpg"),
            hash,
            1_767_225_600_000i64, // 2026-01-01T00:00:00Z
        ],
    )
    .unwrap();
}

#[test]
fn needs_ffmpeg_rows_do_not_claim_a_thumbnail() {
    let conn = db();
    seed_image(&conn, "hdone", Some("2026-01-02T03:04:05.000Z"), "done.jpg");
    seed_image(&conn, "hblocked", Some(preview::NEEDS_FFMPEG), "blocked.jpg");
    seed_image(&conn, "hfailed", Some("failed"), "failed.jpg");

    let items = queries::section_items(&conn, "image", "2026-01", Tz::UTC).unwrap();
    let thumb_of = |hash: &str| {
        items
            .iter()
            .find(|i| i.hash.as_deref() == Some(hash))
            .unwrap_or_else(|| panic!("{hash} missing from the section"))
            .has_thumb
    };

    assert!(thumb_of("hdone"), "a real derive has a thumbnail");
    // The sentinel is written on a path that returns BEFORE any decode, so no
    // cache file exists: claiming a thumbnail renders a broken image where the
    // extension placeholder belongs, for a whole HEIC library.
    assert!(!thumb_of("hblocked"), "a blocked still has no thumbnail");
    assert!(!thumb_of("hfailed"), "a failed derive has no thumbnail");
}

#[test]
fn the_needs_ffmpeg_sentinel_matches_the_literal_the_queries_use() {
    // queries.rs spells the sentinel inline inside its SQL, so a rename of the
    // constant would silently stop excluding it and bring the broken tiles
    // back. This is the pin that makes that a test failure instead.
    assert_eq!(preview::NEEDS_FFMPEG, "needs-ffmpeg");
}

#[test]
fn copy_count_counts_every_live_path_for_one_content() {
    let conn = db();
    seed_image(&conn, "hshared", Some("2026-01-02T03:04:05.000Z"), "a.jpg");
    // Two more copies of the SAME content, as a backup set produces.
    for name in ["b.jpg", "c.jpg"] {
        conn.execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms, \
             content_hash, resolved_utc_ms, resolved_source, date_only, missing, companion_of) \
             VALUES (?1, '/backup', ?2, ?3, 'jpg', 'image', 100, 0, 'hshared', ?4, 'metadata', 0, 0, NULL)",
            params![
                format!("/backup/{name}"),
                name,
                name.trim_end_matches(".jpg"),
                1_767_225_600_000i64
            ],
        )
        .unwrap();
    }

    let items = queries::section_items(&conn, "image", "2026-01", Tz::UTC).unwrap();
    let item = items
        .iter()
        .find(|i| i.hash.as_deref() == Some("hshared"))
        .expect("the logical item");
    // One logical row, three paths — the badge that doubles as a backup health
    // check. Asserted through the real GROUP BY, not a SELECT the test wrote.
    assert_eq!(items.len(), 1, "copies collapse into ONE logical item");
    assert_eq!(item.copy_count, 3);
}

#[test]
fn a_missing_copy_does_not_count_toward_the_badge() {
    let conn = db();
    seed_image(&conn, "hgone", Some("2026-01-02T03:04:05.000Z"), "a.jpg");
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms, \
         content_hash, resolved_utc_ms, resolved_source, date_only, missing, companion_of) \
         VALUES ('/backup/a.jpg', '/backup', 'a.jpg', 'a', 'jpg', 'image', 100, 0, 'hgone', ?1, \
         'metadata', 0, 1, NULL)",
        params![1_767_225_600_000i64],
    )
    .unwrap();

    let items = queries::section_items(&conn, "image", "2026-01", Tz::UTC).unwrap();
    let item = items.iter().find(|i| i.hash.as_deref() == Some("hgone")).unwrap();
    assert_eq!(item.copy_count, 1, "a vanished copy is not a copy");
}

#[test]
fn issues_returns_the_full_total_and_the_newest_rows_within_the_limit() {
    // Several tests assert issue rows were INSERTED; nothing asserted they can
    // be read back, though the Issues view is the app's promise that a silent
    // skip never happens.
    let conn = db();
    for i in 1..=5 {
        conn.execute(
            "INSERT INTO issues (path, kind, message, created_at_utc) \
             VALUES (?1, 'decode-error', ?2, ?3)",
            params![
                format!("/root/{i}.jpg"),
                format!("failure {i}"),
                format!("2026-01-0{i}T00:00:00.000Z")
            ],
        )
        .unwrap();
    }

    let (total, rows) = queries::issues(&conn, 2).unwrap();
    assert_eq!(total, 5, "the total counts every row, not the page");
    assert_eq!(rows.len(), 2, "the limit bounds the page");
    // Newest first: the badge and the list both depend on this order.
    assert_eq!(rows[0].message.as_deref(), Some("failure 5"));
    assert_eq!(rows[1].message.as_deref(), Some("failure 4"));
}

/// Seeds an image in a specific directory at a specific instant.
fn seed_at(conn: &Connection, hash: &str, dir: &str, name: &str, utc_ms: i64) {
    conn.execute(
        "INSERT INTO contents (hash, kind, byte_size, width, height, derived_at_utc, sharpness) \
         VALUES (?1, 'image', 100, 640, 480, '2026-01-02T03:04:05.000Z', ?2)",
        params![hash, (hash.len() as f64)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms, \
         content_hash, resolved_utc_ms, resolved_source, date_only, missing, companion_of) \
         VALUES (?1, ?2, ?3, ?4, 'jpg', 'image', 100, 0, ?5, ?6, 'metadata', 0, 0, NULL)",
        params![
            format!("{dir}/{name}"),
            dir,
            name,
            name.trim_end_matches(".jpg"),
            hash,
            utc_ms
        ],
    )
    .unwrap();
}

#[test]
fn section_dirs_matches_the_directories_of_section_items() {
    // section_dirs converts a month key BACK into a range through the display
    // timezone — a different implementation from the forward bucketing that
    // produced the key. rescan_section is its only consumer, so a disagreement
    // means a rescan quietly walks the wrong directories. Under Asia/Tokyo
    // (UTC+9) these two instants straddle a month boundary that UTC does not
    // see, which is exactly where the two implementations can diverge.
    let conn = db();
    let tz: Tz = "Asia/Tokyo".parse().unwrap();
    // 2026-01-31T20:00:00Z == 2026-02-01T05:00 JST → February in Tokyo.
    seed_at(&conn, "hfeb", "/root/feb", "a.jpg", 1_769_889_600_000);
    // 2026-01-31T10:00:00Z == 2026-01-31T19:00 JST → January in Tokyo.
    seed_at(&conn, "hjan", "/root/jan", "b.jpg", 1_769_853_600_000);

    for month in ["2026-01", "2026-02"] {
        let items = queries::section_items(&conn, "image", month, tz).unwrap();
        let mut expected: Vec<String> = items
            .iter()
            .map(|i| {
                conn.query_row(
                    "SELECT dir_path FROM paths WHERE content_hash = ?1",
                    params![i.hash.as_deref().unwrap()],
                    |r| r.get::<_, String>(0),
                )
                .unwrap()
            })
            .collect();
        expected.sort();
        expected.dedup();

        let mut dirs = queries::section_dirs(&conn, "image", month, tz).unwrap();
        dirs.sort();
        dirs.dedup();
        assert_eq!(dirs, expected, "{month}: the rescan must walk what the section shows");
    }
    // The boundary really did split them, or the test proves nothing.
    assert_eq!(
        queries::section_items(&conn, "image", "2026-02", tz).unwrap().len(),
        1,
        "the Tokyo boundary puts exactly one item in February"
    );
}

#[test]
fn similar_group_of_returns_live_members_best_first_and_drops_the_rest() {
    // What the entire keep-one-delete-the-rest surface renders. Zero tests.
    let conn = db();
    seed_image(&conn, "sharp", Some("2026-01-02T03:04:05.000Z"), "sharp.jpg");
    seed_image(&conn, "soft", Some("2026-01-02T03:04:05.000Z"), "soft.jpg");
    seed_image(&conn, "gone", Some("2026-01-02T03:04:05.000Z"), "gone.jpg");
    conn.execute("UPDATE contents SET sharpness = 9.0 WHERE hash = 'sharp'", [])
        .unwrap();
    conn.execute("UPDATE contents SET sharpness = 1.0 WHERE hash = 'soft'", [])
        .unwrap();
    // Every path of this member vanished from disk.
    conn.execute("UPDATE paths SET missing = 1 WHERE content_hash = 'gone'", [])
        .unwrap();
    conn.execute("INSERT INTO similar_groups (id, created_at_utc) VALUES (1, 'x')", [])
        .unwrap();
    for hash in ["sharp", "soft", "gone"] {
        conn.execute(
            "INSERT INTO similar_group_members (group_id, content_hash) VALUES (1, ?1)",
            params![hash],
        )
        .unwrap();
    }

    let members = queries::similar_group_of(&conn, "sharp").unwrap();
    assert_eq!(
        members.iter().map(|m| m.hash.as_str()).collect::<Vec<_>>(),
        vec!["sharp", "soft"],
        "live members only, sharpest first"
    );
    assert!(members.iter().all(|m| m.copy_count == 1));
    assert!(members.iter().all(|m| m.has_thumb));
}

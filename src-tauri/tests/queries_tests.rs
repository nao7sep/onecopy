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

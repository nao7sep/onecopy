// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).
//
// Change events name the Main sections an owner's writes touched, and Main
// skips re-reading any other section. A section missing here leaves Main
// showing stale items, so these pin both sides of a move, the facts that reach
// an item without moving it, and that nothing is named before it commits.

use chrono_tz::Tz;
use onecopy_lib::index_store;
use onecopy_lib::queries::SectionLocation;
use onecopy_lib::scanner;
use onecopy_lib::section_changes::{self, SectionLog};
use rusqlite::{params, Connection};
use std::collections::HashSet;

const JAN_2026: i64 = 1_767_225_600_000; // 2026-01-01T00:00:00Z
const MAR_2026: i64 = 1_772_323_200_000; // 2026-03-01T00:00:00Z

fn db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-section-changes-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    (dir, conn)
}

fn section(kind: &str, month: &str) -> SectionLocation {
    SectionLocation { kind: kind.to_string(), month: month.to_string() }
}

fn set(sections: &[SectionLocation]) -> HashSet<SectionLocation> {
    sections.iter().cloned().collect()
}

/// One hashed image item dated January 2026 UTC; returns its path id.
fn seed_image(conn: &Connection, hash: &str) -> i64 {
    conn.execute(
        "INSERT INTO contents (hash, kind, byte_size, width, height) VALUES (?1, 'image', 100, 640, 480)",
        [hash],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms, \
         content_hash, resolved_utc_ms, resolved_source) \
         VALUES (?1, '/root', ?2, ?3, 'jpg', 'image', 100, 0, ?3, ?4, 'metadata')",
        params![format!("/root/{hash}.jpg"), format!("{hash}.jpg"), hash, JAN_2026],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn drained(conn: &Connection) -> HashSet<SectionLocation> {
    section_changes::drain(conn, Tz::UTC).unwrap()
}

#[test]
fn a_changed_date_names_the_old_and_the_new_month_once() {
    let (_dir, conn) = db();
    seed_image(&conn, "h1");
    section_changes::track(&conn).unwrap();
    assert!(drained(&conn).is_empty(), "writes before tracking are not recorded");

    conn.execute("UPDATE paths SET resolved_utc_ms = ?1 WHERE content_hash = 'h1'", [MAR_2026])
        .unwrap();

    assert_eq!(
        drained(&conn),
        set(&[section("image", "2026-01"), section("image", "2026-03")])
    );
    assert!(drained(&conn).is_empty(), "a drain clears what it read");
}

#[test]
fn an_unhashed_other_file_names_its_own_section_on_each_side() {
    let (_dir, conn) = db();
    section_changes::track(&conn).unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms) \
         VALUES ('/root/notes.txt', '/root', 'notes.txt', 'notes', 'txt', 'other', 5, 0)",
        [],
    )
    .unwrap();
    assert_eq!(drained(&conn), set(&[section("other", "undated")]));

    conn.execute(
        "UPDATE paths SET resolved_utc_ms = ?1, resolved_source = 'mtime' WHERE file_name = 'notes.txt'",
        [JAN_2026],
    )
    .unwrap();
    assert_eq!(
        drained(&conn),
        set(&[section("other", "undated"), section("other", "2026-01")])
    );

    conn.execute("UPDATE paths SET missing = 1 WHERE file_name = 'notes.txt'", []).unwrap();
    assert_eq!(drained(&conn), set(&[section("other", "2026-01")]));
}

#[test]
fn content_facts_and_companions_name_the_item_they_belong_to() {
    let (_dir, conn) = db();
    let primary = seed_image(&conn, "h1");
    seed_image(&conn, "h2");
    conn.execute("UPDATE paths SET resolved_utc_ms = ?1 WHERE content_hash = 'h2'", [MAR_2026])
        .unwrap();
    section_changes::track(&conn).unwrap();

    conn.execute("UPDATE contents SET width = 4000 WHERE hash = 'h1'", []).unwrap();
    assert_eq!(drained(&conn), set(&[section("image", "2026-01")]));

    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms, companion_of) \
         VALUES ('/root/h1.xmp', '/root', 'h1.xmp', 'h1', 'xmp', 'companion', 5, 0, ?1)",
        [primary],
    )
    .unwrap();
    assert_eq!(drained(&conn), set(&[section("image", "2026-01")]));
}

#[test]
fn a_rolled_back_write_names_nothing() {
    let (_dir, conn) = db();
    seed_image(&conn, "h1");
    section_changes::track(&conn).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    conn.execute("UPDATE paths SET resolved_utc_ms = ?1", [MAR_2026]).unwrap();
    conn.execute_batch("ROLLBACK").unwrap();
    assert!(drained(&conn).is_empty());
}

#[test]
fn a_log_names_a_write_only_once_it_commits_and_keeps_the_whole_run() {
    let (_dir, conn) = db();
    seed_image(&conn, "h1");
    let log = SectionLog::new(Tz::UTC);
    log.track(&conn);

    conn.execute_batch("BEGIN").unwrap();
    conn.execute("UPDATE paths SET resolved_utc_ms = ?1", [MAR_2026]).unwrap();
    assert_eq!(log.changed_since_last(&conn), Some(Vec::new()), "not yet visible to Main");
    conn.execute_batch("COMMIT").unwrap();
    let both = vec![section("image", "2026-01"), section("image", "2026-03")];
    assert_eq!(log.changed_since_last(&conn), Some(both.clone()));
    assert_eq!(log.changed_since_last(&conn), Some(Vec::new()), "named once per progress");
    assert_eq!(log.sections(), None, "a run still tracking is not complete");

    log.finish(&conn);
    assert_eq!(log.sections(), Some(both), "the run keeps everything it named");
}

#[test]
fn a_log_without_tracking_tables_is_unscoped() {
    let (_dir, conn) = db();
    let log = SectionLog::new(Tz::UTC);
    // `finish` without `track` reads tables that do not exist on this
    // connection: the scope is unknown, so it must not read as "nothing".
    log.finish(&conn);
    assert_eq!(log.sections(), None);
}

#[test]
fn a_source_check_names_only_what_its_walk_changed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("notes.txt"), b"notes").unwrap();
    let conn = index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let settings = scanner::settings_from_config(
        Some(&serde_json::json!({ "sourceDirs": [root] })),
        &temp.path().join("data"),
        0,
    );
    let check = || {
        let log = SectionLog::new(Tz::UTC);
        log.track(&conn);
        scanner::run_source_check(&conn, &settings, &|_| {}).unwrap();
        log.finish(&conn);
        log.sections()
    };

    assert_eq!(check(), Some(vec![section("other", "undated")]));
    assert_eq!(check(), Some(Vec::new()), "an unchanged walk changes no section");
    std::fs::remove_file(root.join("notes.txt")).unwrap();
    assert_eq!(check(), Some(vec![section("other", "undated")]));
}

#[test]
fn a_similarity_cohort_reaches_each_display_month_it_spans() {
    let sections = |bucket: &str, tz: Tz| {
        set(&section_changes::image_sections_for_bucket(bucket, tz).unwrap())
    };
    assert_eq!(sections("2026-03", Tz::UTC), set(&[section("image", "2026-03")]));
    // UTC March ends in local April east of Greenwich and starts in local
    // February west of it.
    assert_eq!(
        sections("2026-03", Tz::Asia__Tokyo),
        set(&[section("image", "2026-03"), section("image", "2026-04")])
    );
    assert_eq!(
        sections("2026-03", Tz::America__New_York),
        set(&[section("image", "2026-02"), section("image", "2026-03")])
    );
    assert_eq!(
        sections("2025-12", Tz::Asia__Tokyo),
        set(&[section("image", "2025-12"), section("image", "2026-01")])
    );
    assert_eq!(sections("undated", Tz::Asia__Tokyo), set(&[section("image", "undated")]));
    assert!(section_changes::image_sections_for_bucket("soon", Tz::UTC).is_err());
}

#[test]
fn a_new_dirty_cohort_names_the_images_shown_with_it() {
    let (_dir, conn) = db();
    section_changes::track(&conn).unwrap();
    conn.execute(
        "INSERT INTO similarity_dirty_buckets (bucket, revision) VALUES ('2026-03', 1)",
        [],
    )
    .unwrap();
    assert_eq!(
        section_changes::drain(&conn, Tz::Asia__Tokyo).unwrap(),
        set(&[section("image", "2026-03"), section("image", "2026-04")])
    );
}

#[test]
fn a_month_marked_for_regrouping_twice_keeps_the_owner_writing() {
    // The index's own triggers mark a month's similarity dirty with an
    // upsert. A trigger's conflict clause gives way to the firing statement's,
    // so tracking must never be able to conflict on a repeated month.
    let (_dir, conn) = db();
    section_changes::track(&conn).unwrap();
    seed_image(&conn, "h1");
    seed_image(&conn, "h2");
    conn.execute("UPDATE paths SET resolved_utc_ms = ?1 WHERE content_hash = 'h2'", [JAN_2026 + 1])
        .unwrap();

    assert!(drained(&conn).contains(&section("image", "2026-01")));
}

#[test]
fn a_tracked_full_scan_finishes_and_names_what_it_indexed() {
    let (_dir, conn) = db();
    let root = tempfile::tempdir().unwrap();
    for name in ["a.jpg", "b.jpg", "notes.txt"] {
        std::fs::write(root.path().join(name), name.as_bytes()).unwrap();
    }
    let home = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "sourceDirs": [root.path().to_string_lossy()],
        "defaultTimezone": "UTC",
    });
    let settings = scanner::settings_from_config(Some(&config), home.path(), 0);
    section_changes::track(&conn).unwrap();
    let mut summary = scanner::run_source_check(&conn, &settings, &|_| {}).unwrap();
    scanner::run_index_tail(&conn, &settings, &|_| {}, &mut summary).unwrap();

    assert!(!drained(&conn).is_empty());
}

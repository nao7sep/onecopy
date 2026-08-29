// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).
//
// queries.rs had no test file, yet it produces everything the user actually
// looks at: the grid's rows, the thumbnail flag, the copy count, the section
// directories a rescan walks, and the similar-group membership the whole
// comparison surface renders.

use chrono_tz::Tz;
use rusqlite::{params, Connection};
use onecopy_lib::derived_state;
use onecopy_lib::index_store;
use onecopy_lib::preview;
use onecopy_lib::queries;

struct TestDb {
    _dir: tempfile::TempDir,
    conn: Connection,
}

impl std::ops::Deref for TestDb {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

fn db() -> TestDb {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-queries-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    TestDb { _dir: dir, conn }
}

fn projection() -> queries::ItemProjectionContext {
    queries::ItemProjectionContext {
        capabilities: derived_state::WorkCapabilities {
            ffmpeg: true,
            face_enabled: false,
            face_models: false,
            transcripts: false,
        },
        similarity_dirty: false,
    }
}

/// One image content row with a live path in January 2026 UTC.
fn seed_image(conn: &Connection, hash: &str, derived_at: Option<&str>, name: &str) {
    conn.execute(
        "INSERT INTO contents
           (hash, kind, byte_size, width, height, derived_at_utc, derived_version) \
         VALUES (?1, 'image', 100, 640, 480, ?2, ?3)",
        params![hash, derived_at, preview::DERIVE_VERSION],
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

    let items = queries::section_items(&conn, "image", "2026-01", Tz::UTC, projection()).unwrap();
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
fn item_work_projection_preserves_completed_truth_without_current_tools() {
    let conn = db();
    seed_image(&conn, "photo", Some("2026-01-02T03:04:05.000Z"), "photo.jpg");
    conn.execute_batch(
        "UPDATE contents SET face_score = 0.75 WHERE hash = 'photo';
         INSERT INTO analysis_receipts (content_hash, face_state)
           VALUES ('photo', 'ready');
         INSERT INTO similar_groups (id, created_at_utc) VALUES (1, 'now');
         INSERT INTO similar_group_members (group_id, content_hash) VALUES (1, 'photo');",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO contents
           (hash, kind, byte_size, duration_ms, derived_at_utc, derived_version, strip_frames)
         VALUES ('video', 'video', 200, 60000, 'now', ?1, 4)",
        [derived_state::DERIVE_VERSION],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths
           (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms,
            content_hash, resolved_utc_ms, resolved_source, date_only, missing, companion_of)
         VALUES
           ('/root/video.mov', '/root', 'video.mov', 'video', 'mov', 'video', 200, 0,
            'video', 1767225600000, 'metadata', 0, 0, NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO analysis_receipts (content_hash, transcript_state)
         VALUES ('video', 'ready-text')",
        [],
    )
    .unwrap();

    let projection = queries::ItemProjectionContext {
        capabilities: derived_state::WorkCapabilities {
            ffmpeg: false,
            face_enabled: false,
            face_models: false,
            transcripts: false,
        },
        similarity_dirty: true,
    };
    let photo = queries::item_by_hash(&conn, "photo", projection)
        .unwrap()
        .unwrap();
    assert_eq!(photo.derived_work.preview.as_ref().unwrap().state, "ready");
    assert_eq!(photo.derived_work.faces.as_ref().unwrap().state, "ready");
    assert!(photo.derived_work.faces.as_ref().unwrap().has_value);
    assert_eq!(photo.face_score, Some(0.75));
    assert_eq!(serde_json::to_value(&photo).unwrap()["faceScore"], 0.75);
    assert_eq!(photo.derived_work.similarity.as_ref().unwrap().state, "pending");
    assert!(photo.derived_work.similarity.as_ref().unwrap().has_value);

    let video = queries::item_by_hash(&conn, "video", projection)
        .unwrap()
        .unwrap();
    assert_eq!(video.derived_work.snapshots.as_ref().unwrap().state, "ready");
    assert!(video.derived_work.snapshots.as_ref().unwrap().has_value);
    assert_eq!(video.derived_work.transcripts.as_ref().unwrap().state, "ready");
    assert!(video.derived_work.transcripts.as_ref().unwrap().has_value);

    conn.execute(
        "UPDATE analysis_receipts SET transcript_state = NULL WHERE content_hash = 'video'",
        [],
    )
    .unwrap();
    let unavailable = queries::item_by_hash(&conn, "video", projection)
        .unwrap()
        .unwrap();
    assert_eq!(
        unavailable.derived_work.transcripts.as_ref().unwrap().reason,
        Some("Waiting for ffmpeg")
    );
    let model_missing = queries::item_by_hash(
        &conn,
        "video",
        queries::ItemProjectionContext {
            capabilities: derived_state::WorkCapabilities {
                ffmpeg: true,
                face_enabled: false,
                face_models: false,
                transcripts: false,
            },
            similarity_dirty: false,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        model_missing
            .derived_work
            .transcripts
            .as_ref()
            .unwrap()
            .reason,
        Some("Waiting for transcription model")
    );
}

#[test]
fn stale_preview_is_pending_everywhere_and_is_not_advertised_as_current() {
    let conn = db();
    seed_image(&conn, "stale", Some("2026-01-02T03:04:05.000Z"), "stale.jpg");
    conn.execute(
        "UPDATE contents SET derived_version = ?1 WHERE hash = 'stale'",
        [derived_state::DERIVE_VERSION - 1],
    )
    .unwrap();

    let item = queries::item_by_hash(
        &conn,
        "stale",
        queries::ItemProjectionContext {
            capabilities: derived_state::WorkCapabilities {
                ffmpeg: true,
                face_enabled: true,
                face_models: true,
                transcripts: true,
            },
            similarity_dirty: false,
        },
    )
    .unwrap()
    .unwrap();
    assert!(!item.has_thumb);
    assert_eq!(item.derived_work.preview.as_ref().unwrap().state, "pending");
    assert!(item.derived_work.preview.as_ref().unwrap().has_value);
    assert_eq!(item.derived_work.faces.as_ref().unwrap().state, "waiting");
    assert_eq!(item.derived_work.similarity.as_ref().unwrap().state, "waiting");
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

    let items = queries::section_items(&conn, "image", "2026-01", Tz::UTC, projection()).unwrap();
    let item = items
        .iter()
        .find(|i| i.hash.as_deref() == Some("hshared"))
        .expect("the logical item");
    // One logical row, three physical copies. Asserted through the real
    // projection rather than a count query owned by the test.
    assert_eq!(items.len(), 1, "copies collapse into ONE logical item");
    assert_eq!(item.copy_count, 3);
}

#[test]
fn one_derived_item_can_be_projected_without_reading_its_section() {
    let conn = db();
    seed_image(&conn, "single", Some("2026-01-02T03:04:05.000Z"), "one.jpg");
    conn.execute(
        "UPDATE contents SET width = 4000, height = 3000, sharpness = 9.0
         WHERE hash = 'single'",
        [],
    )
    .unwrap();

    let item = queries::item_by_hash(&conn, "single", projection()).unwrap().unwrap();
    assert_eq!(item.hash.as_deref(), Some("single"));
    assert_eq!((item.width, item.height), (Some(4000), Some(3000)));
    assert_eq!(item.dir_paths, ["/root"]);
    assert!(queries::item_by_hash(&conn, "missing", projection())
        .unwrap()
        .is_none());
}

#[test]
fn a_missing_copy_does_not_count_toward_the_logical_item() {
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

    let items = queries::section_items(&conn, "image", "2026-01", Tz::UTC, projection()).unwrap();
    let item = items.iter().find(|i| i.hash.as_deref() == Some("hgone")).unwrap();
    assert_eq!(item.copy_count, 1, "a vanished copy is not a copy");
}

#[test]
fn logical_summary_tracks_representative_date_name_and_presence_changes() {
    let conn = db();
    seed_image(&conn, "hmoving", Some("2026-02-01T00:00:00.000Z"), "later.jpg");
    conn.execute(
        "UPDATE paths SET resolved_utc_ms = ?1 WHERE content_hash = 'hmoving'",
        [1_769_904_000_000i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, mtime_ms, \
         content_hash, resolved_utc_ms, resolved_source, date_only, missing, companion_of) \
         VALUES ('/backup/early.jpg', '/backup', 'early.jpg', 'early', 'jpg', 'image', 100, 0, \
                 'hmoving', ?1, 'metadata', 0, 0, NULL)",
        params![1_767_225_600_000i64],
    )
    .unwrap();

    let january = queries::section_items(&conn, "image", "2026-01", Tz::UTC, projection()).unwrap();
    assert_eq!(january.len(), 1);
    assert_eq!(january[0].copy_count, 2);
    assert_eq!(january[0].file_name, "early.jpg", "the oldest copy supplies the name");

    conn.execute("UPDATE paths SET missing = 1 WHERE file_name = 'early.jpg'", [])
        .unwrap();
    assert!(queries::section_items(&conn, "image", "2026-01", Tz::UTC, projection())
        .unwrap()
        .is_empty());
    let february = queries::section_items(&conn, "image", "2026-02", Tz::UTC, projection()).unwrap();
    assert_eq!(february.len(), 1, "the remaining copy defines the logical month");
    assert_eq!(february[0].copy_count, 1);
    assert_eq!(february[0].file_name, "later.jpg");
}

#[test]
fn issues_page_oldest_first_with_the_full_total() {
    // The modal's contract: OLDEST first (the longest-standing condition
    // leads — the developer's call), the total counting every row.
    let conn = db();
    for i in 1..=5 {
        conn.execute(
            "INSERT INTO issues (path, kind, message, first_seen_utc, last_seen_utc) \
             VALUES (?1, 'decode-error', ?2, ?3, ?3)",
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
    assert_eq!(rows[0].message.as_deref(), Some("failure 1"));
    assert_eq!(rows[1].message.as_deref(), Some("failure 2"));
}

#[test]
fn a_recurring_issue_is_one_row_whose_last_seen_moves() {
    // The flood the developer reported: the same condition re-recorded on
    // every scan piled up identical rows. Identity is (kind, path) — a
    // recurrence UPDATES, and first-seen keeps the original onset.
    let conn = db();
    index_store::upsert_issue(&conn, Some("/root/a.jpg"), "decode-error", "first failure")
        .unwrap();
    index_store::upsert_issue(&conn, Some("/root/a.jpg"), "decode-error", "same failure again")
        .unwrap();
    // A different KIND on the same path is a different condition.
    index_store::upsert_issue(&conn, Some("/root/a.jpg"), "read-error", "unrelated").unwrap();

    let (total, rows) = queries::issues(&conn, 10).unwrap();
    assert_eq!(total, 2, "recurrence must never insert a second row");
    let decode = rows.iter().find(|r| r.kind == "decode-error").unwrap();
    assert_eq!(decode.message.as_deref(), Some("same failure again"));
    assert!(decode.first_seen_utc <= decode.last_seen_utc);
}

#[test]
fn clearing_retires_only_the_named_kinds_at_the_path() {
    // The success counterpart that makes issues current-state: a scan that
    // finds the condition resolved deletes the row, and only that row.
    let conn = db();
    index_store::upsert_issue(&conn, Some("/root/a.jpg"), "read-error", "x").unwrap();
    index_store::upsert_issue(&conn, Some("/root/a.jpg"), "delete-error", "op record").unwrap();
    index_store::upsert_issue(&conn, Some("/root/b.jpg"), "read-error", "x").unwrap();

    index_store::clear_issues(&conn, "/root/a.jpg", &["read-error", "copies-disagree"]).unwrap();

    let (total, rows) = queries::issues(&conn, 10).unwrap();
    assert_eq!(total, 2);
    // The operation record survives (not re-checkable, waits for dismissal),
    // and the OTHER path's condition is untouched.
    assert!(rows.iter().any(|r| r.kind == "delete-error"));
    assert!(rows.iter().any(|r| r.path.as_deref() == Some("/root/b.jpg")));
}

#[test]
fn filesystem_recovery_is_backend_authored_and_projects_active_work_as_running() {
    let conn = db();
    index_store::upsert_issue(&conn, Some("/root/a.jpg"), "read-error", "unreadable").unwrap();
    let issue_id: i64 = conn
        .query_row("SELECT id FROM issues", [], |row| row.get(0))
        .unwrap();

    let (_, available) = queries::issues(&conn, 10).unwrap();
    let recovery = available[0].recovery.as_ref().unwrap();
    assert_eq!(recovery.action, "recheck");
    assert_eq!(recovery.label, "Recheck");
    assert_eq!(recovery.status, "available");

    let running = onecopy_lib::scan_runtime::try_with_recheck_claim(issue_id, || {
        queries::issues(&conn, 10).unwrap().1
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        running[0].recovery.as_ref().unwrap().status,
        "running",
        "the backend projection follows the admitted index claim"
    );
}

#[test]
fn terminal_derived_worker_failure_offers_an_explicit_restart() {
    let conn = db();
    index_store::upsert_issue(
        &conn,
        None,
        onecopy_lib::issue_recovery::DERIVED_WORKER_FAILED,
        "worker stopped",
    )
    .unwrap();

    let (_, rows) = queries::issues(&conn, 10).unwrap();
    let recovery = rows[0].recovery.as_ref().unwrap();
    assert_eq!(recovery.action, "retry");
    assert_eq!(recovery.label, "Restart");
    assert_eq!(recovery.status, "available");
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
    // Rescan consumes filesystem identity, not display text. Windows stores
    // the verbatim spelling and must receive it back unchanged.
    seed_at(
        &conn,
        "hwin",
        r"\\?\C:\photos",
        "c.jpg",
        1_769_853_600_000,
    );

    for month in ["2026-01", "2026-02"] {
        let items = queries::section_items(&conn, "image", month, tz, projection()).unwrap();
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
        queries::section_items(&conn, "image", "2026-02", tz, projection())
            .unwrap()
            .len(),
        1,
        "the Tokyo boundary puts exactly one item in February"
    );
}

#[test]
fn section_dirs_cover_hashed_and_unhashed_other_files_when_dated_or_undated() {
    let conn = db();
    conn.execute_batch(
        "INSERT INTO contents (hash, kind, byte_size)
           VALUES ('dated-other', 'other', 10), ('undated-other', 'other', 10);
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms,
            resolved_source)
           VALUES
             ('/hashed/dated.bin', '/hashed', 'dated.bin', 'other',
              'dated-other', 1562760000000, 'filesystem'),
             ('/hashed-undated/undated.bin', '/hashed-undated', 'undated.bin',
              'other', 'undated-other', NULL, 'undated'),
             ('/unhashed/dated.pdf', '/unhashed', 'dated.pdf', 'other', NULL,
              1562760000000, 'filesystem'),
             ('/unhashed-undated/undated.pdf', '/unhashed-undated',
              'undated.pdf', 'other', NULL, NULL, 'undated');",
    )
    .unwrap();

    assert_eq!(
        queries::section_dirs(&conn, "other", "2019-07", chrono_tz::UTC).unwrap(),
        vec!["/hashed".to_string(), "/unhashed".to_string()]
    );
    assert_eq!(
        queries::section_dirs(&conn, "other", "undated", chrono_tz::UTC).unwrap(),
        vec![
            "/hashed-undated".to_string(),
            "/unhashed-undated".to_string()
        ]
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

// The verbatim prefix is a FILESYSTEM detail, and the boundary where it must
// come back off is the read that serves the UI — not each call site
// remembering. On Windows `for_fs` is unconditional, so every indexed path is
// stored `\\?\C:\…`, which makes this every path the user sees rather than
// only the deep ones this machinery exists for. These run on every platform
// because `for_display` is a pure string transform; the SPELLING under test is
// Windows's, and pinning it here is what makes the rule testable from a Mac.
#[test]
fn issue_rows_never_carry_the_verbatim_prefix() {
    let conn = db();
    index_store::upsert_issue(&conn, Some(r"\\?\C:\photos\broken.heic"), "decode-error", "x")
        .unwrap();
    index_store::upsert_issue(&conn, Some(r"\\?\UNC\nas\media\clip.mov"), "video-poster-error", "y")
        .unwrap();
    // A rootless issue keeps its empty path as None, which the prefix strip
    // must not disturb.
    index_store::upsert_issue(&conn, None, "walk-error", "z").unwrap();

    let (total, rows) = queries::issues(&conn, 50).unwrap();
    assert_eq!(total, 3);
    let paths: Vec<Option<&str>> = rows.iter().map(|r| r.path.as_deref()).collect();
    assert!(
        paths.contains(&Some(r"C:\photos\broken.heic")),
        "drive-absolute must lose the marker: {paths:?}"
    );
    assert!(
        paths.contains(&Some(r"\\nas\media\clip.mov")),
        "UNC must come back as \\\\server\\share: {paths:?}"
    );
    assert!(paths.contains(&None), "a rootless issue stays None: {paths:?}");
    for path in paths.into_iter().flatten() {
        assert!(!path.starts_with(r"\\?\"), "{path} still leaks the prefix");
    }
}

#[test]
fn the_stored_issue_path_stays_verbatim_so_clearing_still_matches() {
    // Display is a READ-time transform. If it were applied on the way IN, the
    // pipeline's `clear_issues` — which passes the same `abs_path` it wrote —
    // would stop matching and resolved issues would never disappear.
    let conn = db();
    let stored = r"\\?\C:\photos\broken.heic";
    index_store::upsert_issue(&conn, Some(stored), "decode-error", "x").unwrap();

    index_store::clear_issues(&conn, stored, &["decode-error"]).unwrap();

    let (total, _) = queries::issues(&conn, 50).unwrap();
    assert_eq!(total, 0, "the pipeline's own spelling must still clear the row");
}

#[test]
fn equal_dates_choose_the_case_insensitive_path_order_then_exact_path() {
    let conn = db();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('tied', 9, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms)
           VALUES ('/z/first-inserted.jpg', '/z', 'first-inserted.jpg', 'image', 'tied', 1000);
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms)
           VALUES ('/A/Photo.jpg', '/A', 'Photo.jpg', 'image', 'tied', 1000);
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms)
           VALUES ('/a/photo.jpg', '/a', 'photo.jpg', 'image', 'tied', 1000);",
    )
    .unwrap();

    let items = queries::section_items(
        &conn,
        "image",
        "1970-01",
        chrono_tz::UTC,
        projection(),
    )
    .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].file_name, "Photo.jpg");
    assert_eq!(items[0].dir_paths, vec!["/A", "/a", "/z"]);
}

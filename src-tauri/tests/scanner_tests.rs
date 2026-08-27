// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use rusqlite::Connection;
use onecopy_lib::extensions;
use onecopy_lib::resolution::ResolutionConfig;
use onecopy_lib::scanner;
use onecopy_lib::scanner::*;
use onecopy_lib::index_store;

fn lists() -> ScanLists {
    let owned = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    ScanLists {
        images: owned(extensions::IMAGE_EXTENSIONS),
        videos: owned(extensions::VIDEO_EXTENSIONS),
        companions: owned(extensions::COMPANION_EXTENSIONS),
    }
}

fn resolution_config() -> ResolutionConfig {
    ResolutionConfig {
        default_timezone: chrono_tz::Asia::Tokyo,
        good_range_start_year: 1995,
        // The real clock, deliberately: these tests resolve files THIS test
        // just wrote, so their filesystem timestamps are always "now". A
        // frozen now_ms puts the good range's now+1day ceiling in the past
        // the day after it is written, and every filesystem-timestamp
        // resolution silently becomes Undated. (The pure engine's own tests
        // in resolution_tests.rs do freeze it — their evidence is synthetic,
        // so freezing is what makes them deterministic there.)
        now_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after 1970")
            .as_millis() as i64,
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    conn: Connection,
}

fn fixture(label: &str) -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix(&format!("onecopy-scan-{label}-"))
        .tempdir()
        .unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    Fixture {
        _dir: dir,
        root,
        conn,
    }
}

#[test]
fn pending_index_probe_ignores_derived_media_debt() {
    let fx = fixture("pending-probe");

    // Empty index: nothing pending.
    assert!(!pending_index_work_exists(&fx.conn).unwrap());

    // A media path without a content hash is pending work.
    fx.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, missing) \
             VALUES ('/a/x.jpg', '/a', 'x.jpg', 'image', 0)",
            [],
        )
        .unwrap();
    assert!(pending_index_work_exists(&fx.conn).unwrap());

    // Hashing alone leaves metadata evidence pending.
    fx.conn
        .execute_batch(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 1, 'image');
             UPDATE paths SET content_hash = 'h1';",
        )
        .unwrap();
    assert!(pending_index_work_exists(&fx.conn).unwrap());

    // Once index evidence is complete, underived image and video contents do
    // not restart indexing. They belong exclusively to derived_work.
    fx.conn
        .execute_batch(
            "INSERT INTO evidence (path_id, source, raw) \
               SELECT id, 'live-photo-identifier', NULL FROM paths;
             UPDATE paths SET indexed_at_utc = 'done', resolved_source = 'undated';
             INSERT INTO contents (hash, byte_size, kind) VALUES ('v1', 1, 'video');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing)
               VALUES ('/a/v.mov', '/a', 'v.mov', 'video', 'v1', 0);
             INSERT INTO evidence (path_id, source, raw) \
               SELECT id, 'live-photo-identifier', NULL FROM paths WHERE content_hash = 'v1';
             UPDATE paths SET indexed_at_utc = 'done', resolved_source = 'undated' \
               WHERE content_hash = 'v1';",
        )
        .unwrap();
    assert!(!pending_index_work_exists(&fx.conn).unwrap());
}

fn test_cache(f: &Fixture) -> onecopy_lib::preview::CachePaths {
    onecopy_lib::preview::CachePaths::new(f._dir.path().join("cache"))
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn walk_adds_then_skips_unchanged_then_marks_missing() {
    let f = fixture("walk");
    std::fs::write(f.root.join("IMG_20160305_123456.jpg"), b"aaa").unwrap();
    std::fs::write(f.root.join("notes.txt"), b"bbb").unwrap();

    let s1 = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!((s1.added, s1.unchanged, s1.marked_missing), (2, 0, 0));

    // Second pass: everything unchanged.
    let vanished_before_insert = f.root.join("never-indexed.jpg");
    index_store::upsert_issue(
        &f.conn,
        Some(vanished_before_insert.to_string_lossy().as_ref()),
        STAT_ERROR,
        "stat failed before the row could be inserted",
    )
    .unwrap();
    let s2 = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!((s2.added, s2.unchanged), (0, 2));
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM issues WHERE kind = 'stat-error'"),
        0,
        "a complete walk retires a vanished pre-insert stat failure"
    );

    // Delete one file: the row is marked missing, never removed.
    std::fs::remove_file(f.root.join("notes.txt")).unwrap();
    let s3 = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(s3.marked_missing, 1);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE missing = 1"),
        1
    );
}

#[test]
fn an_incomplete_walk_preserves_known_rows_and_keeps_the_root_dirty() {
    let f = fixture("incomplete-walk");
    let known = f.root.join("known.jpg");
    std::fs::write(&known, b"known").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();

    std::fs::remove_dir_all(&f.root).unwrap();
    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert!(stats.errors > 0);
    assert_eq!(stats.marked_missing, 0);
    assert_eq!(count(&f.conn, "SELECT missing FROM paths"), 0);
    assert_eq!(count(&f.conn, "SELECT dirty FROM scan_dirs"), 1);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM issues WHERE kind = 'walk-error'"),
        1
    );

    std::fs::create_dir_all(&f.root).unwrap();
    let repaired = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(repaired.errors, 0);
    assert_eq!(repaired.marked_missing, 1);
    assert_eq!(count(&f.conn, "SELECT dirty FROM scan_dirs"), 0);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM issues"), 0);
}

#[test]
fn filesystem_issue_rechecks_are_exact_and_leave_pipeline_scope_explicit() {
    let f = fixture("issue-recheck");
    let readable = f.root.join("readable.jpg");
    std::fs::write(&readable, b"readable bytes").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let changed_bytes = b"readable bytes after the failed attempt";
    std::fs::write(&readable, changed_bytes).unwrap();

    let readable_path = readable.to_string_lossy().to_string();
    index_store::upsert_issue(&f.conn, Some(&readable_path), READ_ERROR, "read failed").unwrap();
    let read_id: i64 = f
        .conn
        .query_row(
            "SELECT id FROM issues WHERE kind = ?1 AND path = ?2",
            rusqlite::params![READ_ERROR, readable_path],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        recheck_filesystem_issue(&f.conn, read_id, &lists()).unwrap(),
        RecheckOutcome::Resolved { include_walk: false },
        "a successful exact read resumes only the index tail"
    );
    assert_eq!(
        f.conn
            .query_row(
                "SELECT size FROM paths WHERE abs_path = ?1",
                [&readable_path],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        changed_bytes.len() as i64,
        "the probe must refresh path facts before the hash ladder resumes"
    );

    let missing = f.root.join("vanished.jpg");
    let missing_path = missing.to_string_lossy().to_string();
    f.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, missing) \
             VALUES (?1, ?2, 'vanished.jpg', 'image', 0)",
            rusqlite::params![missing_path, f.root.to_string_lossy().as_ref()],
        )
        .unwrap();
    index_store::upsert_issue(&f.conn, Some(&missing_path), STAT_ERROR, "stat failed").unwrap();
    let stat_id: i64 = f
        .conn
        .query_row(
            "SELECT id FROM issues WHERE kind = ?1 AND path = ?2",
            rusqlite::params![STAT_ERROR, missing_path],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        recheck_filesystem_issue(&f.conn, stat_id, &lists()).unwrap(),
        RecheckOutcome::Resolved { include_walk: true },
        "a vanished path needs the normal repairing walk"
    );
    assert_eq!(
        f.conn
            .query_row(
                "SELECT missing FROM paths WHERE abs_path = ?1",
                [&missing_path],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM issues"), 0);
}

#[test]
fn the_ladder_collapses_copies_and_identifies_unique_media_without_reading() {
    let f = fixture("media-hash");
    // Three identical copies in different subdirs, one distinct file.
    for sub in ["a", "b", "c"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("x.jpg"), b"same-bytes").unwrap();
    }
    std::fs::write(f.root.join("unique.jpg"), b"different").unwrap();

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let stats = hash_pending(&f.conn, &test_cache(&f)).unwrap();
    // The colliding three read fully; the unique-size image reads NOTHING
    // and gets a provisional identity (the cache/UI key).
    assert_eq!(stats.full_hashed, 3);
    assert_eq!(stats.provisional_created, 1);

    // One contents row for the three copies, one provisional for the
    // unique file; copy count over a provisional identity is 1.
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 2);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM contents WHERE hash GLOB 'p*'"),
        1
    );
    let copies: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM paths WHERE content_hash = \
             (SELECT content_hash FROM paths WHERE file_name = 'x.jpg' LIMIT 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(copies, 3);
}

#[test]
fn a_late_copy_of_known_content_collapses_and_promotes() {
    let f = fixture("late-copy");
    std::fs::write(f.root.join("first.jpg"), b"same-bytes").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    hash_pending(&f.conn, &test_cache(&f)).unwrap();
    // Unique at first sight: provisional.
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM contents WHERE hash GLOB 'p*'"),
        1
    );

    // A second identical file arrives: the size collision forces BOTH up
    // the ladder — the provisional promotes and the copies collapse.
    std::fs::write(f.root.join("second.jpg"), b"same-bytes").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let stats = hash_pending(&f.conn, &test_cache(&f)).unwrap();
    assert_eq!(stats.full_hashed, 2);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM contents WHERE hash GLOB 'p*'"),
        0,
        "the provisional identity must promote"
    );
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 1);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE content_hash NOT NULL"),
        2
    );
}

#[test]
fn other_files_with_unique_sizes_are_never_read() {
    let f = fixture("other-tier");
    std::fs::write(f.root.join("a.bin"), vec![1u8; 100]).unwrap();
    std::fs::write(f.root.join("b.bin"), vec![2u8; 200]).unwrap();

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let stats = hash_pending(&f.conn, &test_cache(&f)).unwrap();
    assert_eq!(stats.skipped_unique, 2);
    assert_eq!(stats.prehashed, 0);
    assert_eq!(stats.full_hashed, 0);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE content_hash IS NOT NULL"),
        0
    );
}

#[test]
fn size_collisions_among_other_files_get_hashed_and_deduped() {
    let f = fixture("other-dup");
    std::fs::write(f.root.join("copy1.bin"), b"identical-data").unwrap();
    std::fs::write(f.root.join("copy2.bin"), b"identical-data").unwrap();

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let stats = hash_pending(&f.conn, &test_cache(&f)).unwrap();
    assert_eq!(stats.prehashed, 2);
    assert_eq!(stats.full_hashed, 2);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 1);
}

#[test]
fn diverged_copies_surface_as_a_copies_disagree_issue() {
    let f = fixture("disagree");
    // Same size, same 64K edges, different middle — the bit-rot shape.
    let mut a = vec![7u8; 200_000];
    let mut b = vec![7u8; 200_000];
    a[100_000] = 1;
    b[100_000] = 2;
    std::fs::write(f.root.join("rotted1.bin"), &a).unwrap();
    std::fs::write(f.root.join("rotted2.bin"), &b).unwrap();

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let stats = hash_pending(&f.conn, &test_cache(&f)).unwrap();
    assert_eq!(stats.copies_disagree, 1);
    // One row PER FILE — (kind, path) identity needs a real anchor, and
    // naming the disagreeing files is what lets the user act on the finding.
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM issues WHERE kind = 'copies-disagree'"),
        2
    );
    // Both files keep their own distinct contents rows.
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 2);

    // Current-state: a second pass re-detects the same divergence and must
    // UPDATE the same two rows, never pile up more.
    let stats2 = hash_pending(&f.conn, &test_cache(&f)).unwrap();
    let _ = stats2;
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM issues WHERE kind = 'copies-disagree'"),
        2,
        "a recurrence updates rows in place"
    );
}

#[test]
fn resolve_uses_filename_then_filesystem_and_flags_undated() {
    let f = fixture("resolve");
    // No EXIF in these bytes, so the filename is the winning source.
    std::fs::write(f.root.join("IMG_20160305_123456.jpg"), b"not-a-real-jpeg").unwrap();
    // No date anywhere in name or content: filesystem mtime wins.
    std::fs::write(f.root.join("scan.pdf"), b"pdf-ish").unwrap();

    // Nothing resolvable anywhere: no date in the name, and stored filesystem
    // evidence deliberately pushed outside the good range so the last tier
    // rejects it too. Without this file the test asserted `undated == 0` while
    // its name promised the Undated branch — nothing here ever reached it.
    std::fs::write(f.root.join("mystery.bin"), b"who-knows").unwrap();

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    f.conn
        .execute(
            "UPDATE paths SET mtime_ms = 315532800000, birthtime_ms = 315532800000 \
             WHERE file_name = 'mystery.bin'",
            [],
        )
        .unwrap();
    hash_pending(&f.conn, &test_cache(&f)).unwrap();
    extract_pending(&f.conn).unwrap();
    let stats =
        resolve_from_evidence(&f.conn, &resolution_config(), ResolveScope::PendingOnly)
            .unwrap();
    assert_eq!(stats.resolved, 2);
    assert_eq!(stats.undated, 1, "the pre-1995 file must land in Undated");

    let (source, ms): (String, Option<i64>) = f
        .conn
        .query_row(
            "SELECT resolved_source, resolved_utc_ms FROM paths \
             WHERE file_name = 'mystery.bin'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(source, "undated");
    assert_eq!(ms, None, "an undated row carries no time");

    let (source, ms): (String, i64) = f
        .conn
        .query_row(
            "SELECT resolved_source, resolved_utc_ms FROM paths \
             WHERE file_name = 'IMG_20160305_123456.jpg'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(source, "filename");
    // 2016-03-05 12:34:56 JST == 03:34:56 UTC.
    let expected = chrono::NaiveDate::from_ymd_opt(2016, 3, 5)
        .unwrap()
        .and_hms_opt(3, 34, 56)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(ms, expected);

    let source: String = f
        .conn
        .query_row(
            "SELECT resolved_source FROM paths WHERE file_name = 'scan.pdf'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(source, "filesystem");
}

#[test]
fn companions_pair_same_directory_same_stem_only() {
    let f = fixture("pairing");
    let sub = f.root.join("gopro");
    std::fs::create_dir_all(&sub).unwrap();
    // RAW beside its JPEG (case differs — pairing is case-insensitive).
    std::fs::write(f.root.join("IMG_1234.JPG"), b"jpeg").unwrap();
    std::fs::write(f.root.join("img_1234.arw"), b"raw").unwrap();
    // THM beside its MP4.
    std::fs::write(sub.join("GOPR0001.MP4"), b"video").unwrap();
    std::fs::write(sub.join("GOPR0001.THM"), b"thumb").unwrap();
    // Same stem as the JPG but in another directory: must NOT pair.
    std::fs::write(sub.join("IMG_1234.arw"), b"stray raw").unwrap();

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let stats = pair_companions(&f.conn, true).unwrap();
    assert_eq!(stats.paired, 2);

    let paired_to_jpg: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM paths c JOIN paths p ON c.companion_of = p.id \
             WHERE c.file_name = 'img_1234.arw' AND p.file_name = 'IMG_1234.JPG'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(paired_to_jpg, 1);

    let stray_unpaired: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM paths WHERE file_name = 'IMG_1234.arw' \
             AND dir_path LIKE '%gopro' AND companion_of IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stray_unpaired, 1);

    // Idempotent: a second rebuild reports and retains the same two pairs.
    assert_eq!(pair_companions(&f.conn, true).unwrap().paired, 2);

    pair_companions(&f.conn, false).unwrap();
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE companion_of IS NOT NULL"),
        0,
        "the global pairing switch leaves RAW and sidecars independent"
    );
}

#[test]
fn scoped_pairing_repairs_only_the_affected_directory() {
    let f = fixture("pairing-scope");
    let left = f.root.join("left");
    let right = f.root.join("right");
    for dir in [&left, &right] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("IMG.JPG"), b"jpeg").unwrap();
        std::fs::write(dir.join("IMG.ARW"), b"raw").unwrap();
    }
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(pair_companions(&f.conn, true).unwrap().paired, 2);

    std::fs::remove_file(left.join("IMG.JPG")).unwrap();
    onecopy_lib::watcher::restat_dir(&f.conn, &left, &lists()).unwrap();
    let right_before: i64 = f
        .conn
        .query_row(
            "SELECT companion_of FROM paths WHERE dir_path = ?1 AND kind = 'companion'",
            [right.to_string_lossy().as_ref()],
            |row| row.get(0),
        )
        .unwrap();

    let dirs = vec![left.to_string_lossy().to_string()];
    assert_eq!(
        pair_companions_in_dirs(&f.conn, true, &dirs)
            .unwrap()
            .paired,
        0
    );
    let (left_link, right_after): (Option<i64>, i64) = (
        f.conn
            .query_row(
                "SELECT companion_of FROM paths WHERE dir_path = ?1 AND kind = 'companion'",
                [left.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap(),
        f.conn
            .query_row(
                "SELECT companion_of FROM paths WHERE dir_path = ?1 AND kind = 'companion'",
                [right.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .unwrap(),
    );
    assert_eq!(left_link, None, "the vanished primary unpairs locally");
    assert_eq!(
        right_after, right_before,
        "an unrelated cohort is untouched"
    );
}

#[test]
fn scoped_repair_uses_walk_debt_as_its_crash_receipt() {
    let f = fixture("pairing-recovery");
    std::fs::write(f.root.join("IMG.JPG"), b"jpeg").unwrap();
    let settled = settled_root(&f.conn, &f.root).unwrap();
    walk_root(&f.conn, &settled, &lists()).unwrap();
    let roots = vec![f.root.to_string_lossy().to_string()];
    assert!(!walk_owed(&f.conn, &roots).unwrap());

    let marked = begin_scoped_index_repair(&f.conn, &roots).unwrap();
    assert_eq!(marked.len(), 1);
    assert!(
        walk_owed(&f.conn, &roots).unwrap(),
        "an interrupted repair is owed"
    );

    // A second operation did not create this debt and must never clear it.
    let inherited = begin_scoped_index_repair(&f.conn, &roots).unwrap();
    assert!(inherited.is_empty());
    complete_scoped_index_repair(&f.conn, &inherited).unwrap();
    assert!(walk_owed(&f.conn, &roots).unwrap());

    complete_scoped_index_repair(&f.conn, &marked).unwrap();
    assert!(!walk_owed(&f.conn, &roots).unwrap());
}

#[test]
fn duplicate_live_photo_identifiers_never_cross_directory_cohorts() {
    let f = fixture("live-photo-duplicate-trees");
    for dir in ["backup-a", "backup-b"] {
        f.conn
            .execute(
                "INSERT INTO paths (abs_path, dir_path, file_name, stem, kind, missing)
                 VALUES (?1, ?2, 'still.jpg', 'still', 'image', 0),
                        (?3, ?2, 'motion.mov', 'motion', 'video', 0)",
                rusqlite::params![
                    format!("/{dir}/still.jpg"),
                    format!("/{dir}"),
                    format!("/{dir}/motion.mov")
                ],
            )
            .unwrap();
        f.conn
            .execute(
                "INSERT INTO evidence (path_id, source, raw)
                 SELECT id, 'live-photo-identifier', 'stale-id'
                 FROM paths WHERE dir_path = ?1 AND kind = 'video'",
                [format!("/{dir}")],
            )
            .unwrap();
        f.conn
            .execute(
                "INSERT INTO evidence (path_id, source, raw)
                 SELECT id, 'live-photo-identifier', 'shared-id'
                 FROM paths WHERE dir_path = ?1",
                [format!("/{dir}")],
            )
            .unwrap();
    }

    assert_eq!(pair_companions(&f.conn, true).unwrap().paired, 2);
    assert_eq!(
        count(
            &f.conn,
            "SELECT COUNT(*) FROM paths video JOIN paths image
             ON image.id = video.companion_of
             WHERE video.kind = 'video' AND image.dir_path = video.dir_path"
        ),
        2
    );
}

#[test]
#[serial_test::serial(scan_cancel)]
fn cancelled_pairing_preserves_the_previous_complete_projection() {
    let f = fixture("pairing-cancel");
    std::fs::write(f.root.join("IMG.JPG"), b"jpeg").unwrap();
    std::fs::write(f.root.join("IMG.ARW"), b"raw").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    pair_companions(&f.conn, true).unwrap();

    SCAN_CANCEL.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(pair_companions(&f.conn, false).unwrap_err(), CANCELLED);
    SCAN_CANCEL.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        count(
            &f.conn,
            "SELECT COUNT(*) FROM paths WHERE companion_of IS NOT NULL"
        ),
        1,
        "cancellation happens before the atomic projection changes"
    );
}

#[test]
fn corpus_live_photos_pair_by_identifier_not_stem_and_honor_the_toggle() {
    fn source(parts: &[&str]) -> std::path::PathBuf {
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("tests/fixtures/live-photo");
        for part in parts {
            path.push(part);
        }
        path
    }
    let copy = |from: &[&str], to: &std::path::Path| {
        std::fs::copy(source(from), to).unwrap();
    };

    let f = fixture("live-photo-pairing");
    let jpeg = f.root.join("jpeg-valid");
    let heic = f.root.join("heic-valid");
    let invalid = f.root.join("invalid");
    for dir in [&jpeg, &heic, &invalid] {
        std::fs::create_dir_all(dir).unwrap();
    }

    // Deliberately unrelated stems: metadata, never a filename convention,
    // is the Live Photo authority.
    copy(&["jpeg-pair", "key-photo.jpg"], &jpeg.join("still-a.jpg"));
    copy(
        &["jpeg-pair", "paired-video.mov"],
        &jpeg.join("motion-z.mov"),
    );
    copy(&["heic-pair", "key-photo.heic"], &heic.join("still-b.heic"));
    copy(
        &["heic-pair", "paired-video.mov"],
        &heic.join("motion-y.mov"),
    );

    // Same directory is necessary but not sufficient: a different Apple id
    // and a MOV with no Live Photo metadata both remain primary videos.
    copy(&["jpeg-pair", "key-photo.jpg"], &invalid.join("still.jpg"));
    copy(
        &["heic-pair", "paired-video.mov"],
        &invalid.join("mismatched.mov"),
    );
    copy(
        &["unpaired", "video-without-live-metadata.mov"],
        &invalid.join("missing-id.mov"),
    );

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    extract_pending(&f.conn).unwrap();
    let stats = pair_companions(&f.conn, true).unwrap();
    assert_eq!(stats.paired, 2);
    assert_eq!(
        count(
            &f.conn,
            "SELECT COUNT(*) FROM paths WHERE kind = 'video' AND companion_of IS NOT NULL"
        ),
        2
    );
    assert_eq!(
        count(
            &f.conn,
            "SELECT COUNT(*) FROM paths WHERE file_name IN ('mismatched.mov', 'missing-id.mov') \
             AND companion_of IS NULL"
        ),
        2
    );
    assert_eq!(
        count(
            &f.conn,
            "SELECT COUNT(*) FROM paths movie JOIN paths still ON movie.companion_of = still.id \
             WHERE movie.kind = 'video' AND still.kind = 'image' \
               AND movie.dir_path = still.dir_path AND movie.stem != still.stem"
        ),
        2
    );

    pair_companions(&f.conn, false).unwrap();
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE companion_of IS NOT NULL"),
        0
    );
    assert_eq!(pair_companions(&f.conn, true).unwrap().paired, 2);
}

#[test]
fn a_preexisting_index_backfills_live_photo_evidence_once() {
    let f = fixture("live-photo-backfill");
    std::fs::write(f.root.join("old.jpg"), b"not an image").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();

    // Stand in for an index completed by a build that predated Live Photo
    // evidence. The ordinary metadata checkpoint must not suppress the new
    // one-time fact extraction.
    f.conn
        .execute_batch(
            "INSERT INTO contents (hash, byte_size, kind, derived_at_utc) \
               VALUES ('old-hash', 12, 'image', 'done');
             UPDATE paths SET indexed_at_utc = 'already-indexed', content_hash = 'old-hash';",
        )
        .unwrap();
    assert!(pending_index_work_exists(&f.conn).unwrap());
    assert_eq!(extract_pending(&f.conn).unwrap().extracted, 1);
    assert_eq!(
        count(
            &f.conn,
            "SELECT COUNT(*) FROM evidence WHERE source = 'live-photo-identifier' AND raw IS NULL"
        ),
        1
    );
    assert_eq!(extract_pending(&f.conn).unwrap().extracted, 0);
}

#[test]
fn live_photo_repair_candidates_are_a_stable_bounded_page() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-live-photo-repair-page-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    for index in 0..(scanner::LIVE_PHOTO_REPAIR_PAGE_SIZE + 5) {
        conn.execute(
            "INSERT INTO paths
               (abs_path, dir_path, file_name, kind, indexed_at_utc)
             VALUES (?1, '/', ?2, 'image', 'ready')",
            rusqlite::params![format!("/repair-{index}"), format!("repair-{index}")],
        )
        .unwrap();
    }

    let rows = scanner::live_photo_repair_candidates(
        &conn,
        0,
        scanner::LIVE_PHOTO_REPAIR_PAGE_SIZE,
    )
    .unwrap();
    assert_eq!(rows.len(), scanner::LIVE_PHOTO_REPAIR_PAGE_SIZE);
    assert!(rows.windows(2).all(|pair| pair[0].0 < pair[1].0));

    let next = scanner::live_photo_repair_candidates(
        &conn,
        rows.last().unwrap().0,
        scanner::LIVE_PHOTO_REPAIR_PAGE_SIZE,
    )
    .unwrap();
    assert_eq!(next.len(), 5);
    assert!(next[0].0 > rows.last().unwrap().0);
}

#[test]
fn settings_changes_re_resolve_from_evidence_without_file_reads() {
    let f = fixture("re-resolve");
    std::fs::write(f.root.join("IMG_20160305_123456.jpg"), b"not-a-real-jpeg").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    hash_pending(&f.conn, &test_cache(&f)).unwrap();
    extract_pending(&f.conn).unwrap();
    resolve_from_evidence(&f.conn, &resolution_config(), ResolveScope::PendingOnly).unwrap();

    // Delete the file from disk: a re-resolve that needed to re-read it
    // would now fail or go undated. It must not — evidence is in the DB.
    std::fs::remove_file(f.root.join("IMG_20160305_123456.jpg")).unwrap();

    // Switch the default timezone JST → UTC and re-resolve everything.
    let utc_config = ResolutionConfig {
        default_timezone: chrono_tz::UTC,
        ..resolution_config()
    };
    let stats = resolve_from_evidence(&f.conn, &utc_config, ResolveScope::All).unwrap();
    assert_eq!(stats.resolved, 1);

    let ms: i64 = f
        .conn
        .query_row(
            "SELECT resolved_utc_ms FROM paths WHERE file_name = 'IMG_20160305_123456.jpg'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Under UTC the naive 12:34:56 now IS 12:34:56Z (was 03:34:56Z under JST).
    let expected = chrono::NaiveDate::from_ymd_opt(2016, 3, 5)
        .unwrap()
        .and_hms_opt(12, 34, 56)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(ms, expected);
}

#[test]
fn date_resolution_seeks_across_multiple_bounded_pages() {
    let f = fixture("resolve-pages");
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    for index in 0..600 {
        f.conn
            .execute(
                "INSERT INTO paths \
                 (abs_path, dir_path, file_name, kind, mtime_ms, indexed_at_utc, missing) \
                 VALUES (?1, '/virtual', ?2, 'other', ?3, 'ready', 0)",
                rusqlite::params![
                    format!("/virtual/{index}.txt"),
                    format!("{index}.txt"),
                    now_ms
                ],
            )
            .unwrap();
    }

    let stats =
        resolve_from_evidence(&f.conn, &resolution_config(), ResolveScope::PendingOnly).unwrap();
    assert_eq!((stats.resolved, stats.undated), (600, 0));
    assert_eq!(
        count(
            &f.conn,
            "SELECT COUNT(*) FROM paths WHERE resolved_source = 'filesystem'"
        ),
        600
    );
}

#[test]
fn app_trash_directories_are_never_indexed() {
    let f = fixture("trash-skip");
    let trash = f.root.join(".onecopy-trash").join("2026-08-08");
    std::fs::create_dir_all(&trash).unwrap();
    std::fs::write(trash.join("deleted.jpg"), b"gone").unwrap();
    std::fs::write(f.root.join("kept.jpg"), b"here").unwrap();

    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(stats.seen, 1);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths"), 1);
}

#[test]
fn a_companion_unpairs_when_its_primary_disappears() {
    let f = fixture("unpair");
    std::fs::write(f.root.join("IMG.JPG"), b"jpeg-bytes").unwrap();
    std::fs::write(f.root.join("IMG.ARW"), b"raw-bytes").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    pair_companions(&f.conn, true).unwrap();

    let companion_of = |name: &str| -> Option<i64> {
        f.conn
            .query_row(
                "SELECT companion_of FROM paths WHERE file_name = ?1 AND missing = 0",
                rusqlite::params![name],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert!(companion_of("IMG.ARW").is_some(), "paired to start with");

    // The supported out-of-app change: the JPEG is dragged into a subfolder
    // in Finder. Its old row goes missing and a new row appears elsewhere.
    std::fs::create_dir_all(f.root.join("keepers")).unwrap();
    std::fs::rename(f.root.join("IMG.JPG"), f.root.join("keepers").join("IMG.JPG")).unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    pair_companions(&f.conn, true).unwrap();

    // The RAW must return to the other-files section rather than pointing at a
    // vanished primary: every read model filters companion_of IS NULL, so a
    // stale link makes it invisible in every section, count and issue list.
    assert_eq!(
        companion_of("IMG.ARW"),
        None,
        "an orphaned companion must unpair"
    );
}

#[test]
fn replacing_a_provisionally_identified_file_resets_its_content_facts() {
    let f = fixture("provisional-replace");
    // A unique-size video: the ladder never reads it, so it rests on a
    // provisional `p<path_id>` identity — the normal state for videos, since
    // derive_videos_pending never promotes.
    std::fs::write(f.root.join("clip.mov"), vec![7u8; 500]).unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    hash_pending(&f.conn, &test_cache(&f)).unwrap();

    let key: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'clip.mov'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(key.starts_with('p'), "unique-size video is provisional");

    // Stand in for a completed derive: poster, strip and measurements.
    f.conn
        .execute(
            "UPDATE contents SET derived_at_utc = 'done', strip_frames = 5, phash = 1, \
             sharpness = 2.0, duration_ms = 1000 WHERE hash = ?1",
            rusqlite::params![key],
        )
        .unwrap();

    // The user trims the clip in QuickTime and saves over it: same path, new
    // bytes, new length.
    std::fs::write(f.root.join("clip.mov"), vec![9u8; 900]).unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    hash_pending(&f.conn, &test_cache(&f)).unwrap();

    let (size, derived, strip, phash): (i64, Option<String>, Option<i64>, Option<i64>) = f
        .conn
        .query_row(
            "SELECT byte_size, derived_at_utc, strip_frames, phash FROM contents \
             WHERE hash = (SELECT content_hash FROM paths WHERE file_name = 'clip.mov')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();

    assert_eq!(size, 900, "the new file's size, not the old one's");
    assert_eq!(derived, None, "the derive must run again");
    assert_eq!(strip, None, "the old strip must not be inherited");
    assert_eq!(phash, None, "the old appearance must not be inherited");
}

#[test]
fn an_interrupted_walk_is_still_owed_after_the_tail_resumes() {
    let f = fixture("walk-owed");
    let configured_root = f.root.to_string_lossy().to_string();
    // Match run_full_scan: the walk and its checkpoint both use the settled
    // physical-root spelling, while configuration retains the literal bytes.
    let settled = settled_root(&f.conn, &f.root).unwrap();
    let recorded_root = onecopy_lib::winpath::for_fs(&settled)
        .to_string_lossy()
        .to_string();
    let roots = vec![configured_root];

    // Never walked: owed.
    assert!(walk_owed(&f.conn, &roots).unwrap(), "an unwalked root is owed");

    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        std::fs::write(f.root.join(name), name.as_bytes()).unwrap();
    }
    walk_root(&f.conn, &settled, &lists()).unwrap();
    assert!(
        !walk_owed(&f.conn, &roots).unwrap(),
        "a completed walk settles the debt"
    );

    // Exactly the state a cancelled walk leaves behind: walk_root claims the
    // root with dirty = 1 at its start, and only the completion write clears
    // it, so an abort between the two leaves this row. (The global cancel flag
    // is deliberately not used here — it is process-wide, and setting it would
    // abort every other test running in parallel.)
    f.conn
        .execute(
            "UPDATE scan_dirs SET dirty = 1 WHERE root = ?1",
            rusqlite::params![recorded_root],
        )
        .unwrap();
    assert!(
        walk_owed(&f.conn, &roots).unwrap(),
        "an interrupted walk stays owed — the tail cannot recover unread directories"
    );

    // Draining the tail must NOT clear the debt: pending_work_exists is
    // row-level and sees nothing wrong with rows that were never created.
    hash_pending(&f.conn, &test_cache(&f)).unwrap();
    assert!(
        walk_owed(&f.conn, &roots).unwrap(),
        "the tail draining rows does not mean the root was walked"
    );

    // Only a completed walk clears it.
    walk_root(&f.conn, &settled, &lists()).unwrap();
    assert!(
        !walk_owed(&f.conn, &roots).unwrap(),
        "re-walking settles it again"
    );
}

#[test]
fn completed_walk_checkpoint_uses_the_settled_case_identity() {
    let f = fixture("walk-owed-case");
    let configured = std::fs::canonicalize(&f.root).unwrap();
    let recorded = onecopy_lib::winpath::for_fs(std::path::Path::new(
        &configured.to_string_lossy().to_uppercase(),
    ))
    .to_string_lossy()
    .to_string();
    f.conn
        .execute(
            "INSERT INTO scan_dirs (root, last_completed_at_utc, dirty) VALUES (?1, 'done', 0)",
            rusqlite::params![recorded],
        )
        .unwrap();

    let roots = vec![configured.to_string_lossy().to_string()];
    assert!(
        !walk_owed(&f.conn, &roots).unwrap(),
        "the previously settled case spelling owns the completion checkpoint"
    );
}

#[cfg(unix)]
#[test]
fn completed_walk_checkpoint_uses_the_settled_symlink_identity() {
    let f = fixture("walk-owed-symlink");
    let photos = f.root.join("Photos");
    let alias = f.root.join("ConfiguredAlias");
    std::fs::create_dir_all(&photos).unwrap();
    std::os::unix::fs::symlink(&photos, &alias).unwrap();

    let settled = settled_root(&f.conn, &alias).unwrap();
    walk_root(&f.conn, &settled, &lists()).unwrap();

    let roots = vec![alias.to_string_lossy().to_string()];
    assert!(
        !walk_owed(&f.conn, &roots).unwrap(),
        "the configured alias resolves to the completed physical-root checkpoint"
    );
}

#[test]
fn ffmpeg_blocked_stills_never_create_index_debt() {
    let f = fixture("blocked-stills");
    f.conn
        .execute(
            "INSERT INTO contents (hash, byte_size, kind, derived_at_utc) \
             VALUES ('h1', 1, 'image', ?1)",
            rusqlite::params![onecopy_lib::preview::NEEDS_FFMPEG],
        )
        .unwrap();
    f.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing, \
                                indexed_at_utc, resolved_source) \
             VALUES ('/root/a.heic', '/root', 'a.heic', 'image', 'h1', 0, 'done', 'undated')",
            [],
        )
        .unwrap();
    f.conn
        .execute(
            "INSERT INTO evidence (path_id, source, raw) \
             SELECT id, 'live-photo-identifier', NULL FROM paths",
            [],
        )
        .unwrap();

    assert!(!pending_index_work_exists(&f.conn).unwrap());
}

#[test]
fn contents_without_a_live_path_are_not_pending_work() {
    // A contents row whose only path is missing can never be derived, so
    // reporting it as pending drives a no-op resume scan on EVERY launch —
    // which rebuilds all similarity groups each time for nothing.
    let f = fixture("dead-contents");
    f.conn
        .execute(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 1, 'image')",
            [],
        )
        .unwrap();
    f.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing) \
             VALUES ('/gone/a.jpg', '/gone', 'a.jpg', 'image', 'h1', 1)",
            [],
        )
        .unwrap();

    assert!(
        !pending_index_work_exists(&f.conn).unwrap(),
        "an underivable row must not keep the resume firing forever"
    );
}

/// Builds a real JPEG carrying an EXIF APP1 segment.
///
/// Committed binary fixtures are avoided here deliberately: no tool on the
/// build path can WRITE Exif (the image crate encodes pixels only), so a
/// fixture would be opaque and unregenerable. Assembling the block makes every
/// offset visible and the expectations hand-derivable.
fn jpeg_with_exif(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    const MAKE: &[u8] = b"TestCam\0";
    const MODEL: &[u8] = b"Model1\0";
    const TAKEN: &[u8] = b"2016:03:05 12:34:56\0"; // 20 bytes
    const OFFSET: &[u8] = b"+09:00\0"; // 7 bytes

    // Offsets are relative to the start of the TIFF header.
    const IFD0: u32 = 8;
    const EXIF_IFD: u32 = 50; // 8 + (2 + 3*12 + 4)
    const DATA: u32 = 80; // 50 + (2 + 2*12 + 4)
    let make_at = DATA;
    let model_at = make_at + MAKE.len() as u32;
    let taken_at = model_at + MODEL.len() as u32;
    let offset_at = taken_at + TAKEN.len() as u32;

    // tag, type (2 = ASCII, 4 = LONG), count, value-or-offset.
    let entry = |tag: u16, kind: u16, count: u32, value: u32| {
        let mut e = Vec::new();
        e.extend_from_slice(&tag.to_le_bytes());
        e.extend_from_slice(&kind.to_le_bytes());
        e.extend_from_slice(&count.to_le_bytes());
        e.extend_from_slice(&value.to_le_bytes());
        e
    };

    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II\x2a\x00");
    tiff.extend_from_slice(&IFD0.to_le_bytes());
    // IFD0 — entries must be tag-ascending.
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend(entry(0x010F, 2, MAKE.len() as u32, make_at));
    tiff.extend(entry(0x0110, 2, MODEL.len() as u32, model_at));
    tiff.extend(entry(0x8769, 4, 1, EXIF_IFD));
    tiff.extend_from_slice(&0u32.to_le_bytes()); // no IFD1
    // Exif sub-IFD.
    tiff.extend_from_slice(&2u16.to_le_bytes());
    tiff.extend(entry(0x9003, 2, TAKEN.len() as u32, taken_at));
    tiff.extend(entry(0x9011, 2, OFFSET.len() as u32, offset_at));
    tiff.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(tiff.len() as u32, DATA, "the data area starts where declared");
    tiff.extend_from_slice(MAKE);
    tiff.extend_from_slice(MODEL);
    tiff.extend_from_slice(TAKEN);
    tiff.extend_from_slice(OFFSET);

    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff);
    let mut app1 = vec![0xFF, 0xE1];
    app1.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    app1.extend_from_slice(&payload);

    // A real (tiny) JPEG, with the segment spliced in directly after SOI.
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(8, 8)
        .write_to(&mut encoded, image::ImageFormat::Jpeg)
        .unwrap();
    let encoded = encoded.into_inner();
    let mut jpeg = encoded[..2].to_vec(); // SOI
    jpeg.extend_from_slice(&app1);
    jpeg.extend_from_slice(&encoded[2..]);

    let path = dir.join(name);
    std::fs::write(&path, &jpeg).unwrap();
    path
}

#[test]
fn exif_datetime_and_camera_are_extracted_and_win_resolution() {
    // ResolvedSource::Metadata was never produced from a REAL file anywhere in
    // the suite: resolution_tests hand-builds MetadataTimestamp values and the
    // other scanner tests use EXIF-free bytes deliberately. If extraction
    // silently returned None, every photo would re-date to its filesystem
    // timestamp — the whole library landing in the month it was imported.
    let f = fixture("exif");
    // A filename date that would WIN if metadata were missing, so the test
    // distinguishes "metadata was read" from "something resolved".
    jpeg_with_exif(&f.root, "IMG_20200101_010101.jpg");

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    hash_pending(&f.conn, &test_cache(&f)).unwrap();
    extract_pending(&f.conn).unwrap();
    resolve_from_evidence(&f.conn, &resolution_config(), ResolveScope::PendingOnly).unwrap();

    let (source, ms): (String, i64) = f
        .conn
        .query_row(
            "SELECT resolved_source, resolved_utc_ms FROM paths LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(source, "metadata", "in-file metadata outranks the filename");

    // 2016-03-05 12:34:56 +09:00 == 03:34:56 UTC. The OffsetTimeOriginal is a
    // FACT and must win over the configured default timezone.
    let expected = chrono::NaiveDate::from_ymd_opt(2016, 3, 5)
        .unwrap()
        .and_hms_opt(3, 34, 56)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(ms, expected);

    let (make, model): (Option<String>, Option<String>) = f
        .conn
        .query_row(
            "SELECT camera_make, camera_model FROM contents LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    // Camera identity is what partitions a similarity cluster into bursts.
    assert_eq!(make.as_deref(), Some("TestCam"));
    assert_eq!(model.as_deref(), Some("Model1"));
}

#[test]
fn a_retyped_root_capitalisation_does_not_fork_the_index() {
    // paths.abs_path is unique, so the same file reached under two spellings
    // becomes two rows — and the copy-count badge, which doubles as the backup
    // health check, then reports 2 for a file that exists once. Resolving a
    // path does NOT fix its casing on macOS (realpath echoes what it is given
    // on a case-insensitive volume), so the first-seen spelling has to win.
    let f = fixture("root-case");
    let photos = f.root.join("Photos");
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::write(photos.join("a.jpg"), b"bytes").unwrap();

    let first = settled_root(&f.conn, &photos).unwrap();
    walk_root(&f.conn, &first, &lists()).unwrap();
    let rows_after_first: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows_after_first, 1);

    // The same folder, spelled differently — as a hand-edited settings file or
    // a re-typed path would give it. The volume opens it happily.
    let shouted = f.root.join("PHOTOS");
    let settled = settled_root(&f.conn, &shouted).unwrap();
    assert_eq!(
        settled, first,
        "the spelling already on record must win over a new capitalisation"
    );

    walk_root(&f.conn, &settled, &lists()).unwrap();
    let rows_after_second: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        rows_after_second, 1,
        "one physical file must never become two rows"
    );
}

#[test]
fn a_genuinely_different_root_is_not_confused_with_a_known_one() {
    let f = fixture("root-distinct");
    for name in ["Alpha", "Beta"] {
        std::fs::create_dir_all(f.root.join(name)).unwrap();
    }
    let alpha = settled_root(&f.conn, &f.root.join("Alpha")).unwrap();
    walk_root(&f.conn, &alpha, &lists()).unwrap();

    let beta = settled_root(&f.conn, &f.root.join("Beta")).unwrap();
    assert_ne!(beta, alpha, "different roots stay different");
}

#[test]
fn removing_a_root_forgets_its_files_and_their_cache() {
    let f = fixture("forget-root");
    let kept = f.root.join("Kept");
    let dropped = f.root.join("Dropped");
    for dir in [&kept, &dropped] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(kept.join("a.jpg"), b"kept-bytes").unwrap();
    std::fs::write(dropped.join("b.jpg"), b"dropped-bytes").unwrap();
    let cache = test_cache(&f);
    for dir in [&kept, &dropped] {
        walk_root(&f.conn, dir, &lists()).unwrap();
    }
    hash_pending(&f.conn, &cache).unwrap();

    let dropped_hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'b.jpg'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    for path in [cache.thumb(&dropped_hash), cache.preview(&dropped_hash)] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"webp").unwrap();
    }

    let configured = vec![kept.to_string_lossy().to_string()];
    let forgotten = forget_unconfigured_roots(&f.conn, &configured, &cache).unwrap();

    assert_eq!(forgotten, 1);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'b.jpg'"),
        0,
        "the removed root's files leave the index"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'a.jpg'"),
        1,
        "the kept root is untouched"
    );
    assert!(!cache.thumb(&dropped_hash).exists(), "its cache goes too");
    // The file itself is NOT deleted — the app just stopped being its keeper.
    assert!(dropped.join("b.jpg").exists(), "the file stays on disk");
}

#[test]
fn a_root_that_cannot_be_resolved_is_never_forgotten() {
    // The destructive direction. An unplugged drive cannot be canonicalized,
    // and treating that as "the user removed it" would drop the index for
    // every file on that drive. Stale rows are recoverable; that is not.
    let f = fixture("forget-absent");
    let root = f.root.join("Removable");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.jpg"), b"bytes").unwrap();
    let cache = test_cache(&f);
    walk_root(&f.conn, &root, &lists()).unwrap();
    hash_pending(&f.conn, &cache).unwrap();
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths"), 1);

    std::fs::remove_dir_all(&root).unwrap();
    let configured = vec![root.to_string_lossy().to_string()];
    let forgotten = forget_unconfigured_roots(&f.conn, &configured, &cache).unwrap();

    assert_eq!(forgotten, 0, "an absent-but-configured root is not forgotten");
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths"),
        1,
        "its index survives until the drive returns"
    );
}

#[test]
fn a_differently_spelled_configured_root_is_not_forgotten() {
    let f = fixture("forget-spelling");
    let photos = f.root.join("Photos");
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::write(photos.join("a.jpg"), b"bytes").unwrap();
    let cache = test_cache(&f);
    let settled = settled_root(&f.conn, &photos).unwrap();
    walk_root(&f.conn, &settled, &lists()).unwrap();

    let shouted = vec![f.root.join("PHOTOS").to_string_lossy().to_string()];
    let forgotten = forget_unconfigured_roots(&f.conn, &shouted, &cache).unwrap();

    assert_eq!(forgotten, 0, "a capitalisation difference is not a removal");
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths"), 1);
}

#[test]
fn scan_phase_tokens_serialize_to_the_frontend_contract() {
    for (phase, token) in [
        (ScanPhase::Walk, "walk"),
        (ScanPhase::Hash, "hash"),
        (ScanPhase::Extract, "extract"),
        (ScanPhase::Resolve, "resolve"),
        (ScanPhase::Pair, "pair"),
        (ScanPhase::Indexed, "indexed"),
    ] {
        assert_eq!(serde_json::to_value(phase).unwrap(), serde_json::json!(token));
    }

    let snapshot = ScanProgress {
        phase: ScanPhase::Hash,
        done: 3,
        total: 8,
        current_path: Some("/photos/a.mov".to_string()),
        discovered: None,
        bytes_done: Some(25),
        bytes_total: Some(100),
        failures: 1,
        next_phase: Some(ScanPhase::Extract),
    };
    assert_eq!(
        serde_json::to_value(snapshot).unwrap(),
        serde_json::json!({
            "phase": "hash",
            "done": 3,
            "total": 8,
            "currentPath": "/photos/a.mov",
            "discovered": null,
            "bytesDone": 25,
            "bytesTotal": 100,
            "failures": 1,
            "nextPhase": "extract"
        })
    );
}

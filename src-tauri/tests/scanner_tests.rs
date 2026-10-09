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
        audio: owned(extensions::AUDIO_EXTENSIONS),
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

fn stored_path(path: &std::path::Path) -> String {
    onecopy_lib::winpath::for_fs(path)
        .to_string_lossy()
        .to_string()
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
        Some(&stored_path(&vanished_before_insert)),
        STAT_ERROR,
        "stat failed before the row could be inserted",
    )
    .unwrap();
    let s2 = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!((s2.added, s2.unchanged), (0, 2));
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM active_issues WHERE kind = 'stat-error'"),
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
fn complete_walk_republishes_a_shared_logical_item_once_after_one_copy_vanishes() {
    let f = fixture("walk-shared-logical-item");
    let vanished = f.root.join("vanished.jpg");
    let survivor = f._dir.path().join("outside-root.jpg");
    let vanished = stored_path(&vanished);
    let survivor = stored_path(&survivor);
    let root = stored_path(&f.root);
    let outside = stored_path(f._dir.path());
    f.conn
        .execute_batch(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('shared', 3, 'image');",
        )
        .unwrap();
    f.conn
        .execute(
            "INSERT INTO paths
               (abs_path, dir_path, file_name, kind, content_hash, missing,
                resolved_source, resolved_utc_ms)
             VALUES (?1, ?2, 'vanished.jpg', 'image', 'shared', 0,
                     'filesystem', 2000)",
            rusqlite::params![vanished, root],
        )
        .unwrap();
    f.conn
        .execute(
            "INSERT INTO paths
               (abs_path, dir_path, file_name, kind, content_hash, missing,
                resolved_source, resolved_utc_ms)
             VALUES (?1, ?2, 'outside-root.jpg', 'image', 'shared', 0,
                     'filesystem', 1000)",
            rusqlite::params![survivor, outside],
        )
        .unwrap();

    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(stats.marked_missing, 1);
    assert_eq!(
        f.conn
            .query_row(
                "SELECT live_copy_count FROM logical_contents WHERE content_hash = 'shared'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    let representative: String = f
        .conn
        .query_row(
            "SELECT paths.abs_path FROM logical_contents
             JOIN paths ON paths.id = logical_contents.representative_path_id
             WHERE logical_contents.content_hash = 'shared'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(representative, survivor);
    assert_eq!(
        count(
            &f.conn,
            "SELECT COUNT(*) FROM logical_projection_batch"
        ),
        0
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
        count(&f.conn, "SELECT COUNT(*) FROM active_issues WHERE kind = 'walk-error'"),
        1
    );

    std::fs::create_dir_all(&f.root).unwrap();
    let repaired = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(repaired.errors, 0);
    assert_eq!(repaired.marked_missing, 1);
    assert_eq!(count(&f.conn, "SELECT dirty FROM scan_dirs"), 0);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM active_issues"), 0);
    assert!(count(&f.conn, "SELECT COUNT(*) FROM records.issue_events WHERE event = 'resolved'") > 0);
}

#[test]
fn source_restat_refreshes_changed_files_and_retires_missing_path_issues() {
    let f = fixture("issue-recheck");
    let readable = f.root.join("readable.jpg");
    std::fs::write(&readable, b"readable bytes").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let changed_bytes = b"readable bytes after the failed attempt";
    std::fs::write(&readable, changed_bytes).unwrap();

    let readable_path = stored_path(&readable);
    index_store::upsert_issue(&f.conn, Some(&readable_path), READ_ERROR, "read failed").unwrap();
    onecopy_lib::watcher::restat_dir(&f.conn, &f.root, &lists(), &[f.root.to_string_lossy().into_owned()], std::path::Path::new("/onecopy-test-data-root-never-used")).unwrap();
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
    let missing_path = stored_path(&missing);
    f.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, missing) \
             VALUES (?1, ?2, 'vanished.jpg', 'image', 0)",
            rusqlite::params![missing_path, stored_path(&f.root)],
        )
        .unwrap();
    index_store::upsert_issue(&f.conn, Some(&missing_path), STAT_ERROR, "stat failed").unwrap();
    onecopy_lib::watcher::restat_dir(&f.conn, &f.root, &lists(), &[stored_path(&f.root)], std::path::Path::new("/onecopy-test-data-root-never-used")).unwrap();
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
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM active_issues WHERE kind = 'stat-error'"), 0);
    assert!(count(&f.conn, "SELECT COUNT(*) FROM records.issue_events WHERE event = 'resolved'") > 0);
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
fn supported_audio_gets_content_identity_but_stays_in_the_other_section() {
    let f = fixture("audio-identity");
    std::fs::write(f.root.join("voice.m4a"), b"audio-bytes").unwrap();

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let stats = hash_pending(&f.conn, &test_cache(&f)).unwrap();

    assert_eq!(stats.provisional_created, 1);
    let (path_kind, content_kind, section_kind): (String, String, String) = f
        .conn
        .query_row(
            "SELECT p.kind, c.kind, l.kind
             FROM paths p
             JOIN contents c ON c.hash = p.content_hash
             JOIN logical_contents l ON l.content_hash = c.hash
             WHERE p.file_name = 'voice.m4a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(path_kind, "audio");
    assert_eq!(content_kind, "audio");
    assert_eq!(section_kind, "other");
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
        count(&f.conn, "SELECT COUNT(*) FROM active_issues WHERE kind = 'copies-disagree'"),
        2
    );
    // Both files keep their own distinct contents rows.
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 2);

    // OneCopy's own sentence carries a catalogue key so it follows the
    // interface language; only the quantitative detail stays recorded
    // (R5.5 D-L12, D-L13).
    let (message_key, message): (String, String) = f
        .conn
        .query_row(
            "SELECT message_key, message FROM active_issues WHERE kind = 'copies-disagree' LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(message_key, "notice.copiesDisagree");
    assert!(!message.contains("bit rot"), "the recorded detail stays quantitative, not OneCopy's own prose");

    // Current-state: a second pass re-detects the same divergence and must
    // UPDATE the same two rows, never pile up more.
    let stats2 = hash_pending(&f.conn, &test_cache(&f)).unwrap();
    let _ = stats2;
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM active_issues WHERE kind = 'copies-disagree'"),
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

    // (R7-02) Library-wide re-pairing publishes through the batch publisher,
    // never through the per-row projection trigger.
    f.conn
        .execute_batch(
            "CREATE TEMP TABLE unguarded_path_updates (id INTEGER);
             CREATE TEMP TRIGGER count_unguarded_path_updates AFTER UPDATE ON main.paths
             WHEN NOT EXISTS (SELECT 1 FROM main.logical_projection_batch)
             BEGIN INSERT INTO unguarded_path_updates VALUES (NEW.id); END;",
        )
        .unwrap();
    pair_companions(&f.conn, false).unwrap();
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE companion_of IS NOT NULL"),
        0,
        "the global pairing switch leaves RAW and sidecars independent"
    );
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM unguarded_path_updates"), 0);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM logical_contents"),
        count(&f.conn, "SELECT COUNT(*) FROM logical_content_projection"),
        "the published projection matches the relationships"
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
    onecopy_lib::watcher::restat_dir(&f.conn, &left, &lists(), &[left.to_string_lossy().into_owned()], std::path::Path::new("/onecopy-test-data-root-never-used")).unwrap();
    let right_before: i64 = f
        .conn
        .query_row(
            "SELECT companion_of FROM paths WHERE dir_path = ?1 AND kind = 'companion'",
            [stored_path(&right)],
            |row| row.get(0),
        )
        .unwrap();

    let dirs = vec![stored_path(&left)];
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
                [stored_path(&left)],
                |row| row.get(0),
            )
            .unwrap(),
        f.conn
            .query_row(
                "SELECT companion_of FROM paths WHERE dir_path = ?1 AND kind = 'companion'",
                [stored_path(&right)],
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
fn scoped_repair_debt_is_independent_from_walk_debt() {
    let f = fixture("pairing-recovery");
    std::fs::write(f.root.join("IMG.JPG"), b"jpeg").unwrap();
    let settled = settled_root(&f.conn, &f.root).unwrap();
    walk_root(&f.conn, &settled, &lists()).unwrap();
    let roots = vec![f.root.to_string_lossy().to_string()];
    assert!(!walk_owed(&f.conn, &roots).unwrap());

    let marked = begin_scoped_index_repair(&f.conn, &roots).unwrap();
    assert_eq!(marked.len(), 1);
    assert!(!walk_owed(&f.conn, &roots).unwrap());
    assert_eq!(
        count(&f.conn, "SELECT relationship_dirty FROM scan_dirs"),
        1
    );

    // A second operation did not create this debt and must never clear it.
    let inherited = begin_scoped_index_repair(&f.conn, &roots).unwrap();
    assert!(inherited.is_empty());
    complete_scoped_index_repair(&f.conn, &inherited).unwrap();
    assert_eq!(
        count(&f.conn, "SELECT relationship_dirty FROM scan_dirs"),
        1
    );

    complete_scoped_index_repair(&f.conn, &marked).unwrap();
    assert!(!walk_owed(&f.conn, &roots).unwrap());
    assert_eq!(
        count(&f.conn, "SELECT relationship_dirty FROM scan_dirs"),
        0
    );
}

#[test]
fn source_check_leaves_relationship_work_for_the_independent_tail() {
    let f = fixture("split-source-tail");
    std::fs::write(f.root.join("IMG.JPG"), b"jpeg").unwrap();
    let settings = ScanSettings {
        source_dirs: vec![f.root.to_string_lossy().to_string()],
        lists: lists(),
        resolution: resolution_config(),
        pairing_enabled: true,
        cache_root: f._dir.path().join("apphome").join("cache"),
    };

    run_source_check(&f.conn, &settings, &|_| {}).unwrap();
    assert!(pending_index_work_exists(&f.conn).unwrap());
    assert!(!walk_owed(&f.conn, &settings.source_dirs).unwrap());

    let mut summary = ScanSummary::default();
    run_index_tail(&f.conn, &settings, &|_| {}, &mut summary).unwrap();
    assert!(!pending_index_work_exists(&f.conn).unwrap());
    assert!(!walk_owed(&f.conn, &settings.source_dirs).unwrap());
}

#[test]
fn scoped_source_check_repairs_only_the_requested_root_and_retains_other_sources() {
    let f = fixture("scoped-recovery");
    let other = f._dir.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(f.root.join("gone.txt"), b"gone").unwrap();
    std::fs::write(other.join("retained.txt"), b"retained").unwrap();
    let settings = ScanSettings {
        source_dirs: vec![f.root.to_string_lossy().to_string(), other.to_string_lossy().to_string()],
        lists: lists(), resolution: resolution_config(), pairing_enabled: true,
        cache_root: f._dir.path().join("apphome/cache"),
    };
    run_source_check(&f.conn, &settings, &|_| {}).unwrap();
    std::fs::remove_file(f.root.join("gone.txt")).unwrap();
    std::fs::write(f.root.join("new.txt"), b"new").unwrap();
    std::fs::write(other.join("not-yet-discovered.txt"), b"later").unwrap();
    let summary = run_source_check_scoped(&f.conn, &settings, Some(&settings.source_dirs[..1]), &|_| {}).unwrap();
    assert_eq!(summary.roots, 1);
    assert_eq!(summary.failures, 0);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'gone.txt' AND missing = 1"), 1);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name IN ('new.txt', 'retained.txt') AND missing = 0"), 2);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'not-yet-discovered.txt'"), 0);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM scan_dirs"), 2);
}

#[test]
fn failed_scoped_recovery_retains_the_retry_issue_until_that_root_is_checked_successfully() {
    let f = fixture("failed-scoped-recovery");
    let root = f.root.to_string_lossy().to_string();
    let settings = ScanSettings {
        source_dirs: vec![root.clone()], lists: lists(), resolution: resolution_config(),
        pairing_enabled: true, cache_root: f._dir.path().join("apphome/cache"),
    };
    index_store::upsert_issue(&f.conn, Some(&root), "watcher-recovery-failed", "offline").unwrap();
    std::fs::remove_dir(&f.root).unwrap();
    let failed = run_source_check_scoped(&f.conn, &settings, Some(&settings.source_dirs), &|_| {}).unwrap();
    assert_eq!(failed.failures, 1);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM active_issues WHERE kind = 'watcher-recovery-failed'"), 1);
    std::fs::create_dir(&f.root).unwrap();
    let recovered = run_source_check(&f.conn, &settings, &|_| {}).unwrap();
    assert_eq!(recovered.failures, 0);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM active_issues WHERE kind = 'watcher-recovery-failed'"), 0);
}

#[test]
fn source_check_continues_after_an_unavailable_root() {
    let f = fixture("missing-root-continues");
    let missing = f.root.join("Missing");
    let available = f.root.join("Available");
    std::fs::create_dir_all(&available).unwrap();
    std::fs::write(available.join("IMG.JPG"), b"jpeg").unwrap();
    let settings = ScanSettings {
        source_dirs: vec![
            missing.to_string_lossy().to_string(),
            available.to_string_lossy().to_string(),
        ],
        lists: lists(),
        resolution: resolution_config(),
        pairing_enabled: true,
        cache_root: f._dir.path().join("apphome").join("cache"),
    };

    let summary = run_source_check(&f.conn, &settings, &|_| {}).unwrap();

    assert_eq!(summary.roots, 1, "the available root completes independently");
    assert_eq!(summary.failures, 1, "the unavailable root remains explicit");
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths"), 1);
    assert_eq!(
        f.conn
            .query_row(
                "SELECT path FROM active_issues WHERE kind = 'walk-error'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        missing.to_string_lossy()
    );
}

#[test]
fn source_check_skips_only_a_substituted_root() {
    // R3-07, R1-14: the volume-substitution gate is scoped per root inside
    // the walk loop, so a substituted (or unverifiable) root is refused with
    // an Issue while every other configured root still completes normally.
    let f = fixture("substituted-root-skips");
    let healthy = f.root.join("Healthy");
    let substituted = f.root.join("Substituted");
    std::fs::create_dir_all(&healthy).unwrap();
    std::fs::create_dir_all(&substituted).unwrap();
    std::fs::write(healthy.join("IMG.JPG"), b"jpeg").unwrap();
    std::fs::write(substituted.join("IMG2.JPG"), b"jpeg2").unwrap();
    let settings = ScanSettings {
        source_dirs: vec![
            healthy.to_string_lossy().to_string(),
            substituted.to_string_lossy().to_string(),
        ],
        lists: lists(),
        resolution: resolution_config(),
        pairing_enabled: true,
        cache_root: f._dir.path().join("apphome").join("cache"),
    };
    let data_root = settings.data_root().to_path_buf();
    std::fs::create_dir_all(&data_root).unwrap();
    // A recorded identity nothing on this machine can produce, exactly as
    // the direct volume.rs tests simulate a substituted drive.
    onecopy_lib::volume::check_identity(
        &data_root,
        &substituted.to_string_lossy(),
        "not-the-real-volume-identity",
    )
    .unwrap();

    let summary = run_source_check(&f.conn, &settings, &|_| {}).unwrap();

    assert_eq!(summary.roots, 1, "only the healthy root completes its walk");
    assert_eq!(summary.failures, 1, "the substituted root is refused, not silently skipped");
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths"),
        1,
        "only the healthy root's file is indexed"
    );
    let path = f
        .conn
        .query_row(
            "SELECT path FROM active_issues WHERE kind = 'walk-error'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(path, substituted.to_string_lossy());
}

#[test]
fn a_source_containing_the_data_root_never_indexes_the_apps_own_storage() {
    // R6-02: a source root that happens to contain the app's data root (the
    // whole home directory, say) must not index or churn the app's own
    // index, logs, caches and models.
    let f = fixture("data-root-inside-source");
    // Canonicalized up front, exactly like `settled_root` resolves the walked
    // root: a tempdir can sit under a symlinked prefix (e.g. macOS `/var` ->
    // `/private/var`), and the data root must be compared against the same
    // spelling the walk actually uses.
    let root = f.root.canonicalize().unwrap();
    let data_root = root.join(".onecopy");
    std::fs::create_dir_all(data_root.join("cache")).unwrap();
    std::fs::create_dir_all(data_root.join("logs")).unwrap();
    std::fs::write(data_root.join("index.sqlite3"), b"not a photo").unwrap();
    std::fs::write(data_root.join("logs/app.jpg"), b"looks like a photo").unwrap();
    std::fs::write(root.join("real.jpg"), b"an actual photo").unwrap();

    let settings = ScanSettings {
        source_dirs: vec![root.to_string_lossy().to_string()],
        lists: lists(),
        resolution: resolution_config(),
        pairing_enabled: true,
        cache_root: data_root.join("cache"),
    };

    let summary = run_source_check(&f.conn, &settings, &|_| {}).unwrap();

    assert_eq!(summary.roots, 1);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths"),
        1,
        "only the real photo outside the data root is indexed"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'real.jpg'"),
        1
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'app.jpg'"),
        0,
        "a file that merely looks like a photo, but lives under the data root, is never indexed"
    );
}

#[test]
fn apple_double_sidecars_beside_their_real_file_are_never_indexed() {
    // macOS writes `._name` beside `name` on a volume that cannot store
    // extended attributes and resource forks natively (FAT, exFAT, many
    // network shares). It is operating-system metadata, never library
    // content, so it must never become a row a user can select or trash.
    let f = fixture("apple-double");
    std::fs::write(f.root.join("IMG_0001.jpg"), b"photo").unwrap();
    std::fs::write(f.root.join("._IMG_0001.jpg"), b"resource fork").unwrap();

    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(stats.added, 1, "only the real file is indexed");
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = '._IMG_0001.jpg'"),
        0
    );

    // A sidecar left over from before this exclusion existed (or one that
    // slipped in through some other path) must leave the library cleanly on
    // the next walk: marked missing like any other vanished row, no Issue.
    f.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, stem, kind, size, mtime_ms, missing) \
             VALUES (?1, ?2, '._IMG_0001.jpg', '._img_0001', 'other', 0, 0, 0)",
            rusqlite::params![
                stored_path(&f.root.join("._IMG_0001.jpg")),
                stored_path(&f.root)
            ],
        )
        .unwrap();
    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = '._IMG_0001.jpg' AND missing = 1"),
        1,
        "the pre-existing sidecar row leaves the library like any other vanished path"
    );
    assert_eq!(stats.marked_missing, 1);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM active_issues"),
        0,
        "an excluded path is treated as absent, never a failure"
    );

    // Deleting the real file first turns the sidecar into ordinary content
    // (nothing left to attach to), so it is indexed like any other file.
    std::fs::remove_file(f.root.join("IMG_0001.jpg")).unwrap();
    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = '._IMG_0001.jpg' AND missing = 0"),
        1,
        "a lone ._name with no sibling is ordinary content, not metadata"
    );
    let _ = stats;
}

// `.onecopy-stage-<16 hex home fingerprint>-<10 digit pid>-<nanoid>.tmp`; see
// `file_identity::private_stage_file_name`.
const STAGE_PREFIX_LEN: usize = ".onecopy-stage-".len();
const HOME_FIELD_LEN: usize = 16;
const PID_FIELD_LEN: usize = 10;

fn with_pid(name: &str, pid: u32) -> String {
    let pid_start = STAGE_PREFIX_LEN + HOME_FIELD_LEN + 1;
    let pid_end = pid_start + PID_FIELD_LEN;
    format!("{}{pid:0width$}{}", &name[..pid_start], &name[pid_end..], width = PID_FIELD_LEN)
}

fn with_foreign_home(name: &str) -> String {
    let home_start = STAGE_PREFIX_LEN;
    let home_end = home_start + HOME_FIELD_LEN;
    let foreign = "f".repeat(HOME_FIELD_LEN);
    format!("{}{foreign}{}", &name[..home_start], &name[home_end..])
}

/// A pid guaranteed not to be running any more.
fn exited_pid() -> u32 {
    let mut child = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "exit", "0"])
            .spawn()
            .unwrap()
    } else {
        std::process::Command::new("true").spawn().unwrap()
    };
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn leftover_private_staging_from_this_homes_dead_process_is_removed() {
    // A `.onecopy-stage-*.tmp` (or `.onecopy-claim-*.tmp`) leftover naming
    // this application home and a process that has since exited can only be
    // launch-time garbage from a previous process that quitting gave up on
    // at the mutation-quiescence deadline on normal exit
    // (`app_lifecycle::MUTATION_QUIESCE_DEADLINE`). It
    // must never be indexed as library content, and this host's own walk
    // sweeps it away rather than leaking it forever — but only once this
    // process's own identity is actually proven (a settled data root with a
    // readable installation id; `file_identity::is_abandoned_leftover`). This
    // test binary never settles `paths::data_root`, so it cannot exercise the
    // real "swept" outcome end-to-end without a live Tauri startup; that
    // decision is proven directly, with an injected fingerprint, by
    // `file_identity`'s own `this_installations_dead_pid_file_is_swept` unit
    // test (`tests/unit/file_identity.rs`). What this test proves at the
    // walker level is the other, equally load-bearing half: while unsettled,
    // this home's own dead-pid-looking leftover is never indexed AND never
    // removed, exactly like a live or foreign one.
    let f = fixture("private-staging-leftover-dead");
    std::fs::write(f.root.join("IMG_0002.jpg"), b"photo").unwrap();
    let name = with_pid(
        &onecopy_lib::file_identity::private_stage_file_name().unwrap(),
        exited_pid(),
    );
    let leftover = f.root.join(&name);
    std::fs::write(&leftover, b"partial bytes from a killed process").unwrap();
    assert!(onecopy_lib::file_identity::is_private_tmp_name(&leftover));
    assert!(
        !onecopy_lib::file_identity::is_abandoned_leftover(&leftover),
        "this unsettled test process has no proven identity, so nothing looks abandoned to it"
    );

    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();

    assert_eq!(stats.added, 1, "only the real file is indexed");
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name LIKE '.onecopy-stage-%'"),
        0,
        "still never indexed as library content, proven or not"
    );
    assert!(
        leftover.exists(),
        "an unproven leftover is left alone rather than guessed at"
    );
}

#[test]
fn leftover_private_staging_from_a_live_or_foreign_process_is_never_removed() {
    // Two application homes may be configured to see the same shared root
    // (recoverable storage and manual recovery). A live process's own staging file, or a different home's
    // file sitting in a folder this walk also happens to visit, must never
    // be deleted merely because the walk saw its name — 896c22f's
    // unconditional deletion was exactly this defect. Both stay excluded
    // from the index (never library content) but stay on disk.
    let f = fixture("private-staging-leftover-live");
    std::fs::write(f.root.join("IMG_0003.jpg"), b"photo").unwrap();
    let base = onecopy_lib::file_identity::private_stage_file_name().unwrap();
    let live = f.root.join(with_pid(&base, std::process::id()));
    let foreign = f.root.join(with_foreign_home(&with_pid(&base, exited_pid())));
    std::fs::write(&live, b"still being written").unwrap();
    std::fs::write(&foreign, b"another application home's in-progress output").unwrap();

    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();

    assert_eq!(stats.added, 1, "only the real file is indexed");
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name LIKE '.onecopy-stage-%'"),
        0,
        "neither leftover is ever indexed as library content"
    );
    assert!(live.exists(), "a live process's own file is never removed");
    assert!(foreign.exists(), "a different application home's file is never removed");
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

/// (R4.1 finding 1) A same-stem sidecar (an AAE) must attach to the family's
/// still image, never to its Live Photo MOV: the MOV itself pairs to the
/// image, and a companion of a companion would be orphaned by Move and
/// Delete, which only walk one companion level. This holds regardless of
/// which file the walk happened to index first (insertion/id order), and the
/// tie is broken deterministically by path, never by id.
#[test]
fn companion_never_pairs_to_a_live_photo_movie_in_either_id_order() {
    fn run(order: &[&str]) {
        let f = fixture(&format!("live-photo-companion-{}", order.join("-")));
        let insert_row = |file_name: &str, kind: &str| {
            f.conn
                .execute(
                    "INSERT INTO paths (abs_path, dir_path, file_name, stem, kind, missing) \
                     VALUES (?1, '/family', ?2, 'img_1234', ?3, 0)",
                    rusqlite::params![format!("/family/{file_name}"), file_name, kind],
                )
                .unwrap();
        };
        for file_name in order {
            match *file_name {
                "IMG_1234.HEIC" => insert_row(file_name, "image"),
                "IMG_1234.MOV" => insert_row(file_name, "video"),
                "IMG_1234.AAE" => insert_row(file_name, "companion"),
                other => panic!("unexpected fixture entry: {other}"),
            }
        }
        f.conn
            .execute(
                "INSERT INTO evidence (path_id, source, raw)
                 SELECT id, 'live-photo-identifier', 'shared-id'
                 FROM paths WHERE file_name IN ('IMG_1234.HEIC', 'IMG_1234.MOV')",
                [],
            )
            .unwrap();

        assert_eq!(pair_companions(&f.conn, true).unwrap().paired, 2);

        let aae_pairs_to_heic: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM paths aae JOIN paths heic ON aae.companion_of = heic.id \
                 WHERE aae.file_name = 'IMG_1234.AAE' AND heic.file_name = 'IMG_1234.HEIC'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            aae_pairs_to_heic, 1,
            "order {order:?}: the sidecar must pair with the still image, not the Live Photo movie"
        );

        let mov_pairs_to_heic: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM paths mov JOIN paths heic ON mov.companion_of = heic.id \
                 WHERE mov.file_name = 'IMG_1234.MOV' AND heic.file_name = 'IMG_1234.HEIC'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mov_pairs_to_heic, 1, "order {order:?}: the movie still pairs to the image");
    }

    // The finding's exact failure scenario (MOV indexed first) and its
    // reverse both land on the same deterministic, path-ordered result.
    run(&["IMG_1234.MOV", "IMG_1234.AAE", "IMG_1234.HEIC"]);
    run(&["IMG_1234.HEIC", "IMG_1234.AAE", "IMG_1234.MOV"]);
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
    let snapshots = std::cell::RefCell::new(Vec::new());
    let stats = re_resolve_all_with_progress(&f.conn, &utc_config, false, &|progress| {
        snapshots.borrow_mut().push(progress);
    })
    .unwrap();
    assert_eq!(stats.resolved, 1);
    let snapshots = snapshots.into_inner();
    assert_eq!(snapshots.first().unwrap().phase, ScanPhase::Resolve);
    assert_eq!(snapshots.last().unwrap().phase, ScanPhase::Indexed);
    assert!(snapshots.iter().any(|progress| progress.phase == ScanPhase::Pair));

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
fn settings_re_resolution_leaves_absent_rows_as_resumable_debt() {
    let f = fixture("re-resolve-missing");
    let now_ms = resolution_config().now_ms;
    for (id, missing) in [(1, 0), (2, 1)] {
        f.conn
            .execute(
                "INSERT INTO paths \
                 (id, abs_path, dir_path, file_name, kind, mtime_ms, indexed_at_utc, \
                  resolved_utc_ms, resolved_source, missing) \
                 VALUES (?1, ?2, '/virtual', ?3, 'other', ?4, 'ready', ?4, 'filesystem', ?5)",
                rusqlite::params![
                    id,
                    format!("/virtual/{id}.txt"),
                    format!("{id}.txt"),
                    now_ms,
                    missing,
                ],
            )
            .unwrap();
    }

    re_resolve_all_with_progress(&f.conn, &resolution_config(), false, &|_| {}).unwrap();

    let absent_source: Option<String> = f
        .conn
        .query_row(
            "SELECT resolved_source FROM paths WHERE id = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(absent_source, None);
    f.conn
        .execute("UPDATE paths SET missing = 0 WHERE id = 2", [])
        .unwrap();
    assert!(pending_index_work_exists(&f.conn).unwrap());

    let resumed = resolve_from_evidence(
        &f.conn,
        &resolution_config(),
        ResolveScope::PendingOnly,
    )
    .unwrap();
    assert_eq!(resumed.resolved, 1);
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
    let lookalike = f.root.join(".onecopy-trash-notes");
    std::fs::create_dir(&lookalike).unwrap();
    std::fs::write(lookalike.join("kept.jpg"), b"ordinary hidden copy").unwrap();
    std::fs::write(f.root.join(".onecopy-trash.jpg"), b"ordinary filename").unwrap();
    let nested_trash = lookalike.join(".onecopy-trash");
    std::fs::create_dir(&nested_trash).unwrap();
    std::fs::write(nested_trash.join("deleted.jpg"), b"gone too").unwrap();

    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(stats.seen, 3);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths"), 3);
    assert_eq!(stats.errors, 0);
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
            "INSERT INTO contents (hash, byte_size, kind, derive_outcome) \
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

/// Builds a TIFF metadata container, also reusable as a JPEG EXIF payload.
/// Every field and byte offset is explicit so no opaque fixture is required.
fn tiff_with_exif(offset: &[u8]) -> Vec<u8> {
    const MAKE: &[u8] = b"TestCam\0";
    const MODEL: &[u8] = b"Model1\0";
    const TAKEN: &[u8] = b"2016:03:05 12:34:56\0"; // 20 bytes

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
    tiff.extend(entry(0x9011, 2, offset.len() as u32, offset_at));
    tiff.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(tiff.len() as u32, DATA, "the data area starts where declared");
    tiff.extend_from_slice(MAKE);
    tiff.extend_from_slice(MODEL);
    tiff.extend_from_slice(TAKEN);
    tiff.extend_from_slice(offset);
    tiff
}

fn jpeg_with_exif(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let tiff = tiff_with_exif(b"+09:00\0");
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
fn tiff_fallback_reads_the_separate_explicit_offset() {
    use onecopy_lib::metadata::{read_tiff_metadata, MetadataTimestamp};
    let f = fixture("tiff-offset");
    let path = f.root.join("photo.arw");
    let wall = chrono::NaiveDate::from_ymd_opt(2016, 3, 5).unwrap()
        .and_hms_opt(12, 34, 56).unwrap().and_utc().timestamp_millis();
    for (offset, minutes) in [(b"+09:00\0".as_slice(), 540), (b"-05:30\0".as_slice(), -330)] {
        std::fs::write(&path, tiff_with_exif(offset)).unwrap();
        let facts = read_tiff_metadata(&path).unwrap().unwrap();
        assert_eq!(facts.taken, Some(MetadataTimestamp::Absolute { unix_ms: wall - minutes * 60_000 }));
        assert_eq!(facts.make.as_deref(), Some("TestCam"));
    }
    std::fs::write(&path, tiff_with_exif(b"invalid\0")).unwrap();
    assert!(matches!(read_tiff_metadata(&path).unwrap().unwrap().taken, Some(MetadataTimestamp::Naive { .. })));
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
fn removing_a_source_keeps_a_nested_root_still_configured_as_its_own_source() {
    // A folder can be configured as its own source while it also sits inside
    // a different, separately configured source. Removing the outer source
    // must not sweep away the inner one's rows just because its abs_path
    // starts with the same prefix as the outer root's LIKE pattern.
    let f = fixture("forget-root-nested");
    let parent = f.root.join("Parent");
    let child = parent.join("Child");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(parent.join("a.jpg"), b"parent-bytes").unwrap();
    std::fs::write(child.join("b.jpg"), b"child-bytes").unwrap();
    let cache = test_cache(&f);
    walk_root(&f.conn, &parent, &lists()).unwrap();
    walk_root(&f.conn, &child, &lists()).unwrap();
    hash_pending(&f.conn, &cache).unwrap();

    // Only the nested Child root stays configured; Parent is removed.
    let configured = vec![child.to_string_lossy().to_string()];
    let forgotten = forget_unconfigured_roots(&f.conn, &configured, &cache).unwrap();

    assert_eq!(forgotten, 1, "only Parent's own file leaves");
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'a.jpg'"),
        0,
        "the removed parent's own file leaves the index"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'b.jpg'"),
        1,
        "the nested still-configured root keeps its rows"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM scan_dirs"),
        1,
        "only the child root remains configured"
    );
}

#[test]
fn removing_a_root_with_more_orphaned_hashes_than_sqlites_bound_parameter_limit_still_works() {
    // R6-01: orphan collection is a subquery over `batch_touched_hashes`,
    // never one bound parameter per orphaned hash, so a root holding more
    // unique items than SQLite's bound-parameter ceiling can still be
    // forgotten in one pass, instead of rolling back with "too many SQL
    // variables" on every later source check. The ceiling is lowered on this
    // connection, so a few hundred orphans exceed it as 40,000 would exceed
    // the default.
    let f = fixture("forget-root-scale");
    let kept = f.root.join("Kept");
    let dropped = f.root.join("Dropped");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::create_dir_all(&dropped).unwrap();

    const VARIABLE_CEILING: i32 = 32;
    const ORPHAN_COUNT: i64 = 10 * VARIABLE_CEILING as i64;
    f.conn
        .set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_VARIABLE_NUMBER, VARIABLE_CEILING)
        .unwrap();
    let dropped_str = dropped.to_string_lossy().to_string();
    f.conn
        .execute(
            "INSERT INTO scan_dirs (root, configured_root) VALUES (?1, ?1)",
            [&dropped_str],
        )
        .unwrap();
    // Insertion goes through the batch publisher, exactly like a real walk's
    // bulk write, so the per-row logical-projection trigger does not run
    // for every row just to set up this test's fixture.
    index_store::publish_paths_batch(
        &f.conn,
        |_| Ok(()),
        |conn| {
            let mut insert_content = conn
                .prepare_cached("INSERT INTO contents (hash, byte_size, kind) VALUES (?1, 10, 'image')")
                .map_err(|e| e.to_string())?;
            let mut insert_path = conn
                .prepare_cached(
                    "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing) \
                     VALUES (?1, ?2, ?3, 'image', ?4, 0)",
                )
                .map_err(|e| e.to_string())?;
            for i in 0..ORPHAN_COUNT {
                let hash = format!("h{i:08}");
                insert_content.execute([&hash]).map_err(|e| e.to_string())?;
                let path = format!("{dropped_str}/f{i}.jpg");
                insert_path
                    .execute(rusqlite::params![path, dropped_str, format!("f{i}.jpg"), hash])
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        },
    )
    .unwrap();

    let cache = test_cache(&f);
    let configured = vec![kept.to_string_lossy().to_string()];
    let forgotten = forget_unconfigured_roots(&f.conn, &configured, &cache).unwrap();

    assert_eq!(forgotten, ORPHAN_COUNT as u64);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths"),
        0,
        "every row of the removed root leaves the index"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM contents"),
        0,
        "their orphaned contents rows leave too, not just the first ceiling's worth"
    );
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


/// Records, on `conn` only, which projection batch every write to `paths`
/// ran in: each batch the publisher opens gets the next number, and a write
/// outside any batch (which fires the per-row projection trigger) gets 0.
/// The batch publisher opens one batch per IMMEDIATE transaction, so the
/// rows written under one number are what one transaction held the write
/// lock for — a property read from the writes themselves, not from a clock.
fn record_path_writes_by_batch(conn: &Connection) {
    conn.execute_batch(
        "CREATE TEMP TABLE batches_opened (n INTEGER NOT NULL);
         INSERT INTO batches_opened VALUES (0);
         CREATE TEMP TABLE path_writes (batch INTEGER NOT NULL, path_id INTEGER NOT NULL);
         CREATE TEMP TRIGGER batch_opened AFTER INSERT ON main.logical_projection_batch
         BEGIN UPDATE batches_opened SET n = n + 1; END;
         CREATE TEMP TRIGGER path_updated AFTER UPDATE ON main.paths
         BEGIN
           INSERT INTO path_writes
           SELECT CASE WHEN EXISTS (SELECT 1 FROM main.logical_projection_batch) THEN n ELSE 0 END, NEW.id
           FROM batches_opened;
         END;
         CREATE TEMP TRIGGER path_deleted AFTER DELETE ON main.paths
         BEGIN
           INSERT INTO path_writes
           SELECT CASE WHEN EXISTS (SELECT 1 FROM main.logical_projection_batch) THEN n ELSE 0 END, OLD.id
           FROM batches_opened;
         END;",
    )
    .unwrap();
}

/// (batch, distinct paths written in it), one row per batch, unbatched
/// writes as batch 0.
fn path_writes_by_batch(conn: &Connection) -> Vec<(i64, i64)> {
    let mut statement = conn
        .prepare("SELECT batch, COUNT(DISTINCT path_id) FROM path_writes GROUP BY batch ORDER BY batch")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

/// `rows` indexed images under `dir`, each with its own content and an
/// already resolved date, generated inside SQLite under the projection batch
/// the walk publication uses.
fn insert_indexed_images(conn: &Connection, dir: &str, rows: i64) {
    let now_ms = resolution_config().now_ms;
    conn.execute_batch(&format!(
        r#"
        INSERT INTO logical_projection_batch (singleton) VALUES (1);
        WITH RECURSIVE seq(i) AS (
            SELECT 0 UNION ALL SELECT i + 1 FROM seq WHERE i < {last}
        )
        INSERT INTO contents (hash, byte_size, kind)
        SELECT printf('h%06d', i), 1, 'image' FROM seq;
        WITH RECURSIVE seq(i) AS (
            SELECT 0 UNION ALL SELECT i + 1 FROM seq WHERE i < {last}
        )
        INSERT INTO paths
          (abs_path, dir_path, file_name, kind, content_hash, mtime_ms,
           indexed_at_utc, resolved_utc_ms, resolved_source, missing)
        SELECT '{dir}/' || printf('h%06d', i) || '.jpg', '{dir}',
               printf('h%06d', i) || '.jpg', 'image', printf('h%06d', i),
               {now_ms} + i, 'ready', {now_ms} + i, 'filesystem', 0
        FROM seq;
        DELETE FROM logical_projection_batch;
        "#,
        last = rows - 1,
    ))
    .unwrap();
}

#[test]
fn library_wide_settings_change_holds_the_write_lock_only_briefly() {
    // D-H2: removing a configured source root (a library-wide settings
    // change) used to fire the per-row logical-projection trigger for every
    // row under that root — once for the companion-detach UPDATE, again for
    // the path DELETE, then again per orphan in a separate per-hash loop —
    // each recomputing the correlated-subquery projection view, which held
    // the write lock for seconds on a large root. Going through the
    // projection-batch publisher (`forget_unconfigured_roots`) makes the
    // whole removal one IMMEDIATE transaction in which no row write fires
    // that trigger.
    let f = fixture("large-settings-change");
    let root = stored_path(&f.root);
    f.conn
        .execute(
            "INSERT INTO scan_dirs (root, last_completed_at_utc, dirty) VALUES (?1, 'x', 0)",
            [&root],
        )
        .unwrap();
    insert_indexed_images(&f.conn, &root, 50);
    record_path_writes_by_batch(&f.conn);

    forget_unconfigured_roots(&f.conn, &[], &test_cache(&f)).unwrap();

    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths"), 0);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 0);
    assert_eq!(
        path_writes_by_batch(&f.conn),
        [(1, 50)],
        "every row of the removed root goes in one batch, none outside it"
    );
}

#[test]
fn library_wide_date_re_resolution_holds_the_write_lock_only_briefly() {
    // D-H2 (the date-re-resolution follow-up): `re_resolve_all_with_progress`
    // used to invalidate the whole library in one statement and then have
    // `resolve_from_evidence_with_progress` write one row at a time in
    // autocommit — both fire the per-row logical-projection trigger, or
    // rebuild the projection for every touched hash inside one transaction,
    // so on a large library the write lock was held for the whole pass, far
    // past other writers' busy_timeout. Both phases now page their writes
    // through the projection-batch publisher: each transaction writes at
    // most `RESOLVE_PAGE_SIZE` rows, and no row is written outside a batch.
    let rows = 3 * RESOLVE_PAGE_SIZE as i64 + 5;
    let f = fixture("large-date-re-resolution");
    insert_indexed_images(&f.conn, "/library", rows);
    record_path_writes_by_batch(&f.conn);

    re_resolve_all_with_progress(&f.conn, &resolution_config(), false, &|_| {}).unwrap();

    let batches = path_writes_by_batch(&f.conn);
    assert!(
        batches.iter().all(|(batch, _)| *batch > 0),
        "a row was written outside the projection batch: {batches:?}"
    );
    assert!(
        batches.iter().all(|(_, written)| *written <= RESOLVE_PAGE_SIZE as i64),
        "a transaction wrote more than one page: {batches:?}"
    );
    let pages = (rows as usize).div_ceil(RESOLVE_PAGE_SIZE) as i64;
    assert!(
        batches.len() as i64 >= 2 * pages,
        "both the invalidation and the resolve phase page the whole library: {batches:?}"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE resolved_source IS NULL"),
        0,
        "every row is resolved again"
    );
}

#[test]
fn a_concurrent_commit_does_not_fail_a_source_check() {
    // D-H3: the walk's vanished-path publication (and `apply_policy`) used
    // to open a DEFERRED transaction whose first statement is a read, so it
    // took only a read snapshot and later upgraded to a write lock. SQLite
    // can fail that upgrade at once with SQLITE_BUSY_SNAPSHOT the moment
    // another connection has committed since the snapshot was taken — a
    // real conflict, not an ordinary lock wait, so the busy handler every
    // other writer relies on never even runs. IMMEDIATE takes the write
    // lock up front, before any read, so no commit can land in between and
    // no such conflict can arise.
    //
    // This reproduces the exact statement shape the walk publication and
    // `apply_policy` use (SELECT/INSERT read first, UPDATE/INSERT write
    // second) against two ordinary connections, sequenced deterministically
    // instead of racing threads against a busy_timeout window.
    let f = fixture("source-check-vs-concurrent-commit");
    f.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, missing) \
             VALUES ('/a.jpg', '/', 'a.jpg', 'image', 0)",
            [],
        )
        .unwrap();
    let db_path = f._dir.path().join("index.sqlite3");
    let other = index_store::open(&db_path).unwrap();

    // DEFERRED: read first (snapshot taken), then another connection
    // commits, then the write is attempted — SQLite must refuse it at once.
    f.conn
        .execute_batch("BEGIN DEFERRED; SELECT COUNT(*) FROM paths;")
        .unwrap();
    other
        .execute("UPDATE paths SET missing = 1 WHERE id = 1", [])
        .unwrap();
    let deferred_write = f
        .conn
        .execute("UPDATE paths SET missing = 0 WHERE id = 1", []);
    let _ = f.conn.execute_batch("ROLLBACK;");
    assert!(
        deferred_write.is_err(),
        "a DEFERRED read-then-write transaction must be vulnerable to a \
         stale snapshot — if this now succeeds, this test's premise needs \
         revisiting"
    );

    // IMMEDIATE: the write lock is taken before the read runs, so the other
    // connection's write cannot land in between at all — no snapshot, no
    // conflict, no failure.
    other
        .execute("UPDATE paths SET missing = 0 WHERE id = 1", [])
        .unwrap();
    let immediate = rusqlite::Transaction::new_unchecked(
        &f.conn,
        rusqlite::TransactionBehavior::Immediate,
    )
    .unwrap();
    immediate
        .query_row("SELECT COUNT(*) FROM paths", [], |row| row.get::<_, i64>(0))
        .unwrap();
    immediate
        .execute("UPDATE paths SET missing = 1 WHERE id = 1", [])
        .unwrap();
    immediate.commit().unwrap();
    assert_eq!(
        count(&f.conn, "SELECT missing FROM paths WHERE id = 1"),
        1,
        "the IMMEDIATE transaction's write must land"
    );
}

#[test]
fn concurrent_provisional_promotions_never_race_the_contents_row() {
    // D-L1: `promote_identity` used to read `already_known` in autocommit,
    // then do the insert/repoint/delete as separate statements. Two
    // concurrent promoters of the same provisional key could both see
    // `already_known == false` and then both try to INSERT the same real
    // hash into `contents`, one of them failing on the primary key. One
    // IMMEDIATE transaction that re-reads inside the lock makes this a
    // single owner: the second promoter's re-read sees the first's commit.
    let f = fixture("promote-identity-race");
    let cache = test_cache(&f);
    f.conn
        .execute_batch(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('p1', 1, 'image');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
               VALUES ('/a.jpg', '/', 'a.jpg', 'image', 'p1');
             INSERT INTO contents (hash, byte_size, kind) VALUES ('p2', 1, 'image');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
               VALUES ('/b.jpg', '/', 'b.jpg', 'image', 'p2');",
        )
        .unwrap();

    let db_path = f._dir.path().join("index.sqlite3");
    let cache2 = onecopy_lib::preview::CachePaths::new(f._dir.path().join("cache"));
    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let start2 = start.clone();
    let worker = std::thread::spawn(move || {
        let conn = index_store::open(&db_path).unwrap();
        start2.wait();
        scanner::promote_identity(&conn, &cache2, "p1", "real")
    });

    start.wait();
    let result_main = scanner::promote_identity(&f.conn, &cache, "p2", "real");
    let result_worker = worker.join().unwrap();

    assert!(result_main.is_ok(), "{result_main:?}");
    assert!(result_worker.is_ok(), "{result_worker:?}");
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM contents WHERE hash = 'real'"),
        1,
        "both promotions must merge into exactly one real contents row"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE content_hash = 'real'"),
        2,
        "both paths must end up pointing at the real hash"
    );
}

#[test]
fn zero_byte_files_carry_no_content_identity() {
    let f = fixture("zero-byte");
    for name in ["a.txt", "b.txt", "x.jpg", "y.jpg"] {
        std::fs::write(f.root.join(name), b"").unwrap();
    }
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    let cache = test_cache(&f);
    let stats = hash_pending(&f.conn, &cache).unwrap();
    assert_eq!(stats.prehashed, 0);
    assert_eq!(stats.full_hashed, 0);
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE content_hash IS NULL"),
        2,
        "empty Other files stay unhashed individual files"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(DISTINCT content_hash) FROM paths WHERE content_hash GLOB 'p*'"),
        2,
        "each empty image keeps its own provisional identity"
    );

    // Delivering or decoding an empty image yields the hash of no bytes; it
    // must not promote into a shared identity.
    let provisional: String = f
        .conn
        .query_row("SELECT content_hash FROM paths WHERE file_name = 'x.jpg'", [], |r| r.get(0))
        .unwrap();
    let empty = blake3::hash(b"").to_hex().to_string();
    onecopy_lib::scanner::promote_identity(&f.conn, &cache, &provisional, &empty).unwrap();
    let other: String = f
        .conn
        .query_row("SELECT content_hash FROM paths WHERE file_name = 'y.jpg'", [], |r| r.get(0))
        .unwrap();
    onecopy_lib::scanner::promote_identity(&f.conn, &cache, &other, &empty).unwrap();
    assert_eq!(
        count(&f.conn, "SELECT COUNT(DISTINCT content_hash) FROM paths WHERE file_name IN ('x.jpg', 'y.jpg')"),
        2
    );
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents WHERE byte_size = 0 AND hash NOT GLOB 'p*'"), 0);
}

#[cfg(unix)]
#[test]
fn an_unavailable_root_known_by_another_spelling_is_kept_and_a_removed_one_is_not() {
    // A root configured through a symlink (like a Windows mapped drive) is
    // indexed under its resolved spelling. While its drive is away that
    // spelling cannot be derived from the configuration, so only the walk's
    // own record of which configured root produced it keeps it.
    let f = fixture("forget-offline-alias");
    let drive = f.root.join("Drive");
    let photos = drive.join("Photos");
    let removed = f.root.join("Removed");
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::create_dir_all(&removed).unwrap();
    std::fs::write(photos.join("a.jpg"), b"on the drive").unwrap();
    std::fs::write(removed.join("b.jpg"), b"removed source").unwrap();
    let link = f._dir.path().join("photos-link");
    std::os::unix::fs::symlink(&photos, &link).unwrap();
    let settings = |dirs: &[&std::path::Path]| ScanSettings {
        source_dirs: dirs.iter().map(|dir| dir.to_string_lossy().to_string()).collect(),
        lists: lists(),
        resolution: resolution_config(),
        pairing_enabled: true,
        cache_root: f._dir.path().join("apphome").join("cache"),
    };
    run_source_check(&f.conn, &settings(&[&link, &removed]), &|_| {}).unwrap();
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths"), 2);

    // The drive goes away and the user removes the other source.
    std::fs::rename(&drive, f.root.join("Unplugged")).unwrap();
    let cache = test_cache(&f);
    let configured = vec![link.to_string_lossy().to_string()];
    let forgotten = forget_unconfigured_roots(&f.conn, &configured, &cache).unwrap();

    assert_eq!(forgotten, 1, "only the removed source is forgotten");
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'a.jpg'"),
        1,
        "the unavailable drive keeps its index"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'b.jpg'"),
        0
    );
}

/// Whether any configured root still owes a full walk — never walked to
/// completion, or interrupted — read from the settled root's checkpoint.
fn walk_owed(conn: &rusqlite::Connection, roots: &[String]) -> Result<bool, String> {
    for root in roots {
        let settled = settled_root(conn, std::path::Path::new(root))?;
        let fs_root = onecopy_lib::winpath::for_fs(&settled);
        let complete: Option<bool> = rusqlite::OptionalExtension::optional(conn.query_row(
            "SELECT last_completed_at_utc IS NOT NULL AND dirty = 0 \
             FROM scan_dirs WHERE root = ?1",
            rusqlite::params![fs_root.to_string_lossy().as_ref()],
            |r| r.get(0),
        ))
        .map_err(|e| e.to_string())?;
        if complete != Some(true) {
            return Ok(true);
        }
    }
    Ok(false)
}

// The walk joins every entry onto the root's resolved spelling (verbatim
// `\\?\C:\…` on Windows, symlinks resolved everywhere), so the data root it
// excludes must be compared in that same spelling, not as configured.
#[cfg(unix)]
#[test]
fn the_walk_excludes_the_data_root_however_it_is_spelled() {
    let f = fixture("data-root-spelling");
    let link = f._dir.path().join("link-to-root");
    std::os::unix::fs::symlink(&f.root, &link).unwrap();
    let data_root = f.root.join("apphome");
    std::fs::create_dir_all(data_root.join("cache")).unwrap();
    std::fs::write(data_root.join("cache").join("thumb.jpg"), b"cache").unwrap();
    std::fs::write(f.root.join("photo.jpg"), b"photo").unwrap();
    let settings = ScanSettings {
        source_dirs: vec![f.root.to_string_lossy().to_string()],
        lists: lists(),
        resolution: resolution_config(),
        pairing_enabled: true,
        // The app knows its data root by another spelling of the same folder.
        cache_root: link.join("apphome").join("cache"),
    };

    run_source_check(&f.conn, &settings, &|_| {}).unwrap();

    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'thumb.jpg'"),
        0,
        "the app's own storage is never source content"
    );
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name = 'photo.jpg'"),
        1
    );
}

// A source whose drive stops answering mid-walk: the walk gives up within its
// bound (so whoever holds the index claim gets it back), publishes no
// absence, and keeps the root owed for the next check.
#[test]
fn a_walk_the_drive_stops_answering_ends_incomplete_within_its_bound() {
    use onecopy_lib::volume_io::{FakeStallingVolume, Op};
    let f = fixture("stalled-walk");
    std::fs::write(f.root.join("a.jpg"), b"a").unwrap();
    std::fs::create_dir(f.root.join("zz")).unwrap();
    std::fs::write(f.root.join("zz").join("b.jpg"), b"b").unwrap();
    walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths WHERE missing = 0"), 2);

    let volume = FakeStallingVolume::mount(&f.root, std::time::Duration::from_millis(300));
    volume.stall(&[Op::List], Some(&f.root.join("zz")));
    let started = std::time::Instant::now();
    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    assert!(stats.errors > 0);
    assert_eq!(stats.marked_missing, 0);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths WHERE missing = 0"), 2);
    assert_eq!(count(&f.conn, "SELECT dirty FROM scan_dirs"), 1);

    // While the drive has not answered, the next walk fails fast.
    let started = std::time::Instant::now();
    let again = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_millis(200));
    assert!(again.errors > 0 && again.marked_missing == 0);

    volume.release();
    assert!(volume.wait_until_settled(std::time::Duration::from_secs(5)));
    let repaired = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(repaired.errors, 0);
    assert_eq!(count(&f.conn, "SELECT dirty FROM scan_dirs"), 0);
}

// A source check whose walk found every file as it was leaves companion
// relationships alone; one that changed anything owes them again.
#[test]
fn an_unchanged_walk_owes_no_companion_pass_but_a_changed_one_does() {
    let f = fixture("unchanged-walk-pairing");
    std::fs::write(f.root.join("IMG.JPG"), b"jpeg").unwrap();
    let settings = ScanSettings {
        source_dirs: vec![f.root.to_string_lossy().to_string()],
        lists: lists(),
        resolution: resolution_config(),
        pairing_enabled: true,
        cache_root: f._dir.path().join("apphome").join("cache"),
    };
    run_source_check(&f.conn, &settings, &|_| {}).unwrap();
    let mut summary = ScanSummary::default();
    run_index_tail(&f.conn, &settings, &|_| {}, &mut summary).unwrap();
    assert_eq!(count(&f.conn, "SELECT relationship_dirty FROM scan_dirs"), 0);

    run_source_check(&f.conn, &settings, &|_| {}).unwrap();
    assert_eq!(count(&f.conn, "SELECT relationship_dirty FROM scan_dirs"), 0, "nothing changed");

    std::fs::write(f.root.join("IMG.XMP"), b"sidecar").unwrap();
    run_source_check(&f.conn, &settings, &|_| {}).unwrap();
    assert_eq!(count(&f.conn, "SELECT relationship_dirty FROM scan_dirs"), 1, "a file was added");
}

#[test]
fn the_system_time_zone_follows_the_computer_and_a_chosen_one_stays() {
    let home = tempfile::tempdir().unwrap();
    let computer = iana_time_zone::get_timezone().ok().and_then(|zone| zone.parse::<chrono_tz::Tz>().ok()).unwrap_or(chrono_tz::UTC);
    for config in [serde_json::json!({}), serde_json::json!({ "defaultTimezone": "system" })] {
        let settings = settings_from_config(Some(&config), home.path(), 0);
        assert_eq!(settings.resolution.default_timezone, computer, "{config}");
    }
    let chosen = settings_from_config(Some(&serde_json::json!({ "defaultTimezone": "Pacific/Auckland" })), home.path(), 0);
    assert_eq!(chosen.resolution.default_timezone, chrono_tz::Pacific::Auckland);
}

#[cfg(windows)]
#[test]
fn the_walk_does_not_follow_a_junction() {
    // A junction to a folder inside the same root would index every file
    // under it twice, as two copies of itself.
    let f = fixture("walk-junction");
    let photos = f.root.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    std::fs::write(photos.join("a.jpg"), b"one photo").unwrap();
    let junction = f.root.join("again");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&photos)
        .status()
        .unwrap();
    assert!(made.success(), "mklink /J failed");

    walk_root(&f.conn, &f.root, &lists()).unwrap();

    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE missing = 0"),
        1,
        "only the real file is indexed, not its view through the junction"
    );
}

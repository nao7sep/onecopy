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
        count(&f.conn, "SELECT COUNT(*) FROM issues WHERE kind = 'walk-error'"),
        1
    );

    std::fs::create_dir_all(&f.root).unwrap();
    let repaired = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(repaired.errors, 0);
    assert_eq!(repaired.marked_missing, 1);
    assert_eq!(count(&f.conn, "SELECT dirty FROM scan_dirs"), 0);
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM active_issues"), 0);
    assert!(count(&f.conn, "SELECT COUNT(*) FROM issues WHERE closure = 'resolved'") > 0);
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
    assert!(count(&f.conn, "SELECT COUNT(*) FROM issues WHERE closure = 'resolved'") > 0);
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
        count(&f.conn, "SELECT COUNT(*) FROM issues WHERE kind = 'copies-disagree'"),
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
            "SELECT message_key, message FROM issues WHERE kind = 'copies-disagree' LIMIT 1",
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
                "SELECT path FROM issues WHERE kind = 'walk-error'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        missing.to_string_lossy()
    );
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
        count(&f.conn, "SELECT COUNT(*) FROM issues"),
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

#[test]
fn leftover_private_staging_files_are_never_indexed_and_are_removed() {
    // A `.onecopy-stage-*.tmp` (or `.onecopy-claim-*.tmp`) leftover can only
    // be launch-time garbage from a previous process that quitting gave up
    // on at the mutation-quiescence deadline (`app_lifecycle::MUTATION_QUIESCE_DEADLINE`,
    // `specs/file-operations.md` "Normal exit and abnormal termination"). It
    // must never be indexed as library content, and it is swept away outright
    // rather than left to leak forever.
    let f = fixture("private-staging-leftover");
    std::fs::write(f.root.join("IMG_0002.jpg"), b"photo").unwrap();
    let leftover = f.root.join(".onecopy-stage-abandoned.tmp");
    std::fs::write(&leftover, b"partial bytes from a killed process").unwrap();
    assert!(onecopy_lib::file_identity::is_private_tmp_name(&leftover));

    let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();

    assert_eq!(stats.added, 1, "only the real file is indexed");
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM paths WHERE file_name LIKE '.onecopy-stage-%'"),
        0
    );
    assert!(
        !leftover.exists(),
        "the leftover private staging file is removed, not merely hidden from the index"
    );
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
fn offset_evidence_upgrade_resumes_metadata_without_rehashing_or_losing_results() {
    use onecopy_lib::metadata::MetadataTimestamp;
    let Fixture { _dir, root, conn } = fixture("offset-upgrade");
    let file = root.join("photo.tif");
    std::fs::write(&file, tiff_with_exif(b"+09:00\0")).unwrap();
    walk_root(&conn, &root, &lists()).unwrap();
    let cache = onecopy_lib::preview::CachePaths::new(_dir.path().join("cache"));
    hash_pending(&conn, &cache).unwrap();
    extract_pending(&conn).unwrap();
    resolve_from_evidence(&conn, &resolution_config(), ResolveScope::PendingOnly).unwrap();
    let hash: String = conn.query_row("SELECT content_hash FROM paths", [], |row| row.get(0)).unwrap();
    let naive = serde_json::to_string(&MetadataTimestamp::Naive {
        year: 2016, month: 3, day: 5, hour: 12, minute: 34, second: 56,
    }).unwrap();
    conn.execute("UPDATE evidence SET raw = ?1, offset_known = 0 WHERE source = 'metadata'", [&naive]).unwrap();
    conn.execute_batch("UPDATE contents SET derived_at_utc = 'preserved', face_score = 0.8;
        UPDATE paths SET resolved_source = 'metadata'; PRAGMA user_version = 13;").unwrap();
    index_store::upsert_issue(&conn, None, "fixture-issue", "retained").unwrap();
    drop(conn);

    let db = _dir.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    assert!(pending_index_work_exists(&conn).unwrap());
    assert_eq!(conn.query_row("SELECT date_state FROM logical_contents", [], |row| row.get::<_, String>(0)).unwrap(), "pending");
    assert_eq!(hash_pending(&conn, &cache).unwrap().full_hashed, 0);
    // Another open before completion cannot consume the durable repair debt.
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    assert_eq!(extract_pending(&conn).unwrap().extracted, 1);
    let mut config = resolution_config();
    config.default_timezone = chrono_tz::America::New_York;
    resolve_from_evidence(&conn, &config, ResolveScope::PendingOnly).unwrap();
    let expected = chrono::NaiveDate::from_ymd_opt(2016, 3, 5).unwrap()
        .and_hms_opt(3, 34, 56).unwrap().and_utc().timestamp_millis();
    assert_eq!(conn.query_row("SELECT content_hash, resolved_utc_ms FROM paths", [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))).unwrap(), (hash, expected));
    assert_eq!(conn.query_row("SELECT derived_at_utc, face_score FROM contents", [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))).unwrap(), ("preserved".into(), 0.8));
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    assert_eq!(extract_pending(&conn).unwrap().extracted, 0);
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
    // unique items than SQLite's default 32,766 bound-parameter ceiling can
    // still be forgotten in one pass, instead of rolling back with "too many
    // SQL variables" on every later source check.
    let f = fixture("forget-root-scale");
    let kept = f.root.join("Kept");
    let dropped = f.root.join("Dropped");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::create_dir_all(&dropped).unwrap();

    const ORPHAN_COUNT: i64 = 40_000;
    let dropped_str = dropped.to_string_lossy().to_string();
    f.conn
        .execute(
            "INSERT INTO scan_dirs (root, configured_root) VALUES (?1, ?1)",
            [&dropped_str],
        )
        .unwrap();
    // Insertion goes through the batch publisher, exactly like a real walk's
    // bulk write, so the per-row logical-projection trigger does not run
    // 40,000 times just to set up this test's fixture.
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
        "their orphaned contents rows leave too, not just the first 32,766"
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


#[test]
fn library_wide_settings_change_holds_the_write_lock_only_briefly() {
    // D-H2: removing a configured source root (a library-wide settings
    // change) used to fire the per-row logical-projection trigger for every
    // row under that root — once for the companion-detach UPDATE, again for
    // the path DELETE, then again per orphan in a separate per-hash loop —
    // each recomputing the correlated-subquery projection view. On a large
    // fixture that holds the write lock for seconds. Going through the
    // projection-batch publisher (`forget_unconfigured_roots`) keeps the
    // whole removal one brief IMMEDIATE transaction, so a concurrent
    // preview write started at the same instant still lands quickly.
    const ROWS: i64 = 6_000;
    let f = fixture("large-settings-change");
    let now_ms = resolution_config().now_ms;
    // A single set-based generator, not ROWS round trips from Rust: a
    // recursive CTE produces the rows entirely inside SQLite, under the
    // same projection-batch guard the walk publication uses, so building
    // this fixture is not itself an O(rows) trigger cascade — that per-row
    // cost is real and already sound (ordinary one-row-at-a-time indexing),
    // but it is not what this test measures. Every row gets its own unique
    // hash, so removing the root also orphans every one of them.
    f.conn
        .execute_batch(&format!(
            r#"
            INSERT INTO scan_dirs (root, last_completed_at_utc, dirty) VALUES ('{root}', 'x', 0);
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
            SELECT '{root}/' || printf('h%06d', i) || '.jpg', '{root}',
                   printf('h%06d', i) || '.jpg', 'image', printf('h%06d', i),
                   {now_ms} + i, 'ready', {now_ms} + i, 'filesystem', 0
            FROM seq;
            DELETE FROM logical_projection_batch;
            "#,
            root = stored_path(&f.root),
            last = ROWS - 1,
            now_ms = now_ms,
        ))
        .unwrap();
    // A distinct row outside the removed root, so the probe's write has
    // somewhere valid to land both before and after the removal.
    f.conn
        .execute(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('probe', 1, 'image')",
            [],
        )
        .unwrap();
    f.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
             VALUES ('/elsewhere/probe.jpg', '/elsewhere', 'probe.jpg', 'image', 'probe')",
            [],
        )
        .unwrap();

    let db_path = f._dir.path().join("index.sqlite3");
    let cache = test_cache(&f);
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_done = done.clone();
    let worker = std::thread::spawn(move || {
        forget_unconfigured_roots(&f.conn, &[], &cache).unwrap();
        worker_done.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    // Sample a small, independent write throughout the whole removal: a
    // single well-timed attempt can simply win the race and finish before
    // the worker's first write, proving nothing either way. Continuous
    // sampling instead measures the worst wait any concurrent writer would
    // actually see while the removal is in flight.
    let probe = index_store::open(&db_path).unwrap();
    let mut max_write_ms: u128 = 0;
    let mut samples = 0u32;
    while !done.load(std::sync::atomic::Ordering::SeqCst) {
        let started = std::time::Instant::now();
        onecopy_lib::derived_state::record_preview_success(
            &probe, "probe", "/elsewhere/probe.jpg", 10, 10, 0.5, 42,
        )
        .unwrap();
        max_write_ms = max_write_ms.max(started.elapsed().as_millis());
        samples += 1;
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
    worker.join().unwrap();

    assert!(samples > 0, "the probe never got a chance to run");
    assert!(
        max_write_ms < 1000,
        "a concurrent preview write took {max_write_ms} ms — removing a \
         source root must go through the projection-batch publisher and \
         hold the write lock only briefly"
    );
}

#[test]
fn library_wide_date_re_resolution_holds_the_write_lock_only_briefly() {
    // D-H2 (the date-re-resolution follow-up): `re_resolve_all_with_progress`
    // used to invalidate the whole library in one statement and then have
    // `resolve_from_evidence_with_progress` write `resolved_stmt`/
    // `undated_stmt` one row at a time in autocommit — both fire the
    // per-row logical-projection trigger, or rebuild the projection for
    // every touched hash inside a single transaction. Either way, on a
    // library of hundreds of thousands of rows that holds the write lock
    // for as long as the whole pass takes, far past other writers'
    // busy_timeout. Both the invalidation and the per-file resolve phase
    // now page their writes: each `RESOLVE_PAGE_SIZE` page is computed
    // outside any transaction and published through
    // `index_store::publish_paths_batch` in its own brief IMMEDIATE
    // transaction, so a concurrent preview-success write started at any
    // point during the whole call still succeeds within its own
    // busy_timeout, instead of racing one lock held for the entire pass.
    const ROWS: i64 = 6_000;
    let f = fixture("large-date-re-resolution");
    let now_ms = resolution_config().now_ms;
    // Set-based generation, not ROWS round trips from Rust, under the same
    // projection-batch guard the walk publication uses — building the
    // fixture is not itself the O(rows) autocommit cascade this test
    // measures. Every row already has a resolved date so `re_resolve_all`
    // has to reset and re-resolve all of them from filesystem mtime.
    f.conn
        .execute_batch(&format!(
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
            SELECT '/library/' || printf('h%06d', i) || '.jpg', '/library',
                   printf('h%06d', i) || '.jpg', 'image', printf('h%06d', i),
                   {now_ms} + i, 'ready', {now_ms} + i, 'filesystem', 0
            FROM seq;
            DELETE FROM logical_projection_batch;
            "#,
            last = ROWS - 1,
            now_ms = now_ms,
        ))
        .unwrap();
    // A distinct row for the probe's write to land on, unaffected by the
    // re-resolve so its content never changes.
    f.conn
        .execute(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('probe', 1, 'image')",
            [],
        )
        .unwrap();
    f.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
             VALUES ('/elsewhere/probe.jpg', '/elsewhere', 'probe.jpg', 'image', 'probe')",
            [],
        )
        .unwrap();

    let db_path = f._dir.path().join("index.sqlite3");
    let config = resolution_config();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_done = done.clone();
    let worker = std::thread::spawn(move || {
        re_resolve_all_with_progress(&f.conn, &config, false, &|_| {}).unwrap();
        worker_done.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    // Sample a small, independent write throughout the whole re-resolve.
    // SQLite's default busy handler backs off on a fixed schedule, not a
    // randomized one; against a worker whose page transactions run back to
    // back at a steady cadence, that fixed schedule can stay in step with
    // the worker's cadence and miss every gap for seconds at a time even
    // though brief gaps keep recurring throughout the run — the
    // "WAL-fairness flakiness" a previous attempt at this test saw. A
    // custom handler with a randomized retry interval breaks that
    // resonance; it gives up (returning `false`, so the call fails fast
    // with SQLITE_BUSY) after a bounded number of retries rather than
    // blocking for the app's full 5 s busy_timeout.
    fn jittered_busy_handler(count: i32) -> bool {
        if count > 150 {
            return false;
        }
        let jitter_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0) as u64
            % 3_000;
        std::thread::sleep(std::time::Duration::from_micros(500 + jitter_ns));
        true
    }
    let probe = index_store::open(&db_path).unwrap();
    probe.busy_handler(Some(jittered_busy_handler)).unwrap();
    let start = std::time::Instant::now();
    let mut successes = 0u32;
    let mut attempts = 0u32;
    let mut last_success_at = start;
    let mut max_gap_ms: u128 = 0;
    while !done.load(std::sync::atomic::Ordering::SeqCst) {
        attempts += 1;
        if onecopy_lib::derived_state::record_preview_success(
            &probe, "probe", "/elsewhere/probe.jpg", 10, 10, 0.5, 42,
        )
        .is_ok()
        {
            successes += 1;
            let now = std::time::Instant::now();
            max_gap_ms = max_gap_ms.max(now.duration_since(last_success_at).as_millis());
            last_success_at = now;
        }
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
    max_gap_ms = max_gap_ms.max(std::time::Instant::now().duration_since(last_success_at).as_millis());
    worker.join().unwrap();

    // Without the fix, invalidation ran as one transaction spanning the
    // whole library and the per-file resolve phase wrote one autocommit
    // statement per row back to back with no real idle gap: whichever
    // connection just released the write lock is already running and
    // reacquires it before a woken competitor can, so every short-window
    // attempt started during either stretch fails in a row. Averaging over
    // the whole run hides this — a short pathological stretch is diluted by
    // a longer friendly one — so the real signal is the longest stretch
    // with zero successful writes, not the overall success rate. With the
    // fix, every page's transaction is followed by a real idle interval, so
    // no such stretch approaches the length either phase used to hold the
    // lock for.
    assert!(
        attempts >= 10,
        "the probe only got {attempts} chances to run"
    );
    assert!(successes > 0, "the probe never got a single write through");
    assert!(
        max_gap_ms < 1_000,
        "the probe went {max_gap_ms} ms without a single successful write \
         during the re-resolve — the invalidation and per-file resolve \
         phases must page their writes through the projection-batch \
         publisher, with a real idle interval between pages, so no single \
         transaction (or unbroken run of them) holds the write lock long \
         enough to starve a concurrent writer"
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

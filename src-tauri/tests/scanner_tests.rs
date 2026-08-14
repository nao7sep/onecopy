// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use std::path::Path;
use rusqlite::Connection;
use onecopy_lib::extensions;
use onecopy_lib::resolution::ResolutionConfig;
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
fn pending_work_probe_tracks_unhashed_media_and_underived_contents() {
    let fx = fixture("pending-probe");

    // Empty index: nothing pending.
    assert!(!pending_work_exists(&fx.conn, true).unwrap());

    // A media path without a content hash is pending work.
    fx.conn
        .execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, missing) \
             VALUES ('/a/x.jpg', '/a', 'x.jpg', 'image', 0)",
            [],
        )
        .unwrap();
    assert!(pending_work_exists(&fx.conn, false).unwrap());

    // Hashed but underived image content: still pending.
    fx.conn
        .execute_batch(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 1, 'image');
             UPDATE paths SET content_hash = 'h1';",
        )
        .unwrap();
    assert!(pending_work_exists(&fx.conn, false).unwrap());

    // Derived image: clean. An underived video counts only when ffmpeg
    // is present — a resume that could do nothing must not fire.
    fx.conn
        .execute_batch(
            "UPDATE contents SET derived_at_utc = 'done';
             INSERT INTO contents (hash, byte_size, kind) VALUES ('v1', 1, 'video');",
        )
        .unwrap();
    assert!(!pending_work_exists(&fx.conn, false).unwrap());
    assert!(pending_work_exists(&fx.conn, true).unwrap());
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
    let s2 = walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!((s2.added, s2.unchanged), (0, 2));

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
    assert_eq!(
        count(&f.conn, "SELECT COUNT(*) FROM issues WHERE kind = 'copies-disagree'"),
        1
    );
    // Both files keep their own distinct contents rows.
    assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 2);
}

#[test]
fn resolve_uses_filename_then_filesystem_and_flags_undated() {
    let f = fixture("resolve");
    // No EXIF in these bytes, so the filename is the winning source.
    std::fs::write(f.root.join("IMG_20160305_123456.jpg"), b"not-a-real-jpeg").unwrap();
    // No date anywhere in name or content: filesystem mtime wins.
    std::fs::write(f.root.join("scan.pdf"), b"pdf-ish").unwrap();

    walk_root(&f.conn, &f.root, &lists()).unwrap();
    hash_pending(&f.conn, &test_cache(&f)).unwrap();
    extract_pending(&f.conn).unwrap();
    let stats =
        resolve_from_evidence(&f.conn, &resolution_config(), ResolveScope::PendingOnly)
            .unwrap();
    assert_eq!(stats.resolved, 2);
    assert_eq!(stats.undated, 0);

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
    let stats = pair_companions(&f.conn).unwrap();
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

    // Idempotent: a second pass pairs nothing new.
    assert_eq!(pair_companions(&f.conn).unwrap().paired, 0);
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

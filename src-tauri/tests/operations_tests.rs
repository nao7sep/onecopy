// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use rusqlite::Connection;
use onecopy_lib::preview::CachePaths;
use onecopy_lib::operations::*;
use onecopy_lib::index_store;
use onecopy_lib::scanner::{self, ScanLists};
use onecopy_lib::extensions;

struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    app_root: std::path::PathBuf,
    cache: CachePaths,
    conn: Connection,
}

fn fixture(label: &str) -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix(&format!("onecopy-ops-{label}-"))
        .tempdir()
        .unwrap();
    let root = dir.path().join("root");
    let app_root = dir.path().join("apphome");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&app_root).unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    Fixture {
        _dir: dir,
        root,
        app_root,
        cache,
        conn,
    }
}

fn lists() -> ScanLists {
    let owned = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    ScanLists {
        images: owned(extensions::IMAGE_EXTENSIONS),
        videos: owned(extensions::VIDEO_EXTENSIONS),
        companions: owned(extensions::COMPANION_EXTENSIONS),
    }
}

fn scan(f: &Fixture) {
    scanner::walk_root(&f.conn, &f.root, &lists()).unwrap();
    scanner::hash_pending(&f.conn, &f.cache).unwrap();
    scanner::pair_companions(&f.conn).unwrap();
}

#[test]
fn deleting_a_logical_item_trashes_every_copy_and_companion() {
    let f = fixture("cascade");
    for sub in ["a", "b"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("x.jpg"), b"same-bytes").unwrap();
    }
    // A companion RAW beside copy a.
    std::fs::write(f.root.join("a").join("x.arw"), b"raw-bytes").unwrap();
    scan(&f);

    let hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'x.jpg' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let outcome = delete_item(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        DeleteMode::Trash,
    )
    .unwrap();
    assert_eq!(outcome.deleted_files, 3, "two copies + one companion");
    assert_eq!(outcome.failed_files, 0);

    // Disk: originals gone, all three RECOVERABLE in the app-root trash. The
    // comment claimed this and nothing asserted it — the trash being the only
    // safety net, "deleted 3" agreeing with itself is not evidence.
    assert!(!f.root.join("a").join("x.jpg").exists());
    assert!(!f.root.join("b").join("x.jpg").exists());
    assert!(!f.root.join("a").join("x.arw").exists());

    let day_dir = std::fs::read_dir(f.app_root.join("trash"))
        .expect("the trash root exists")
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("one day folder");
    let manifest: Vec<serde_json::Value> =
        std::fs::read_to_string(day_dir.join("manifest.jsonl"))
            .expect("a manifest was written")
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
    assert_eq!(manifest.len(), 3, "one manifest line per trashed file");
    for line in &manifest {
        let stored = line["storedPath"].as_str().expect("storedPath");
        let original = line["originalPath"].as_str().expect("originalPath");
        assert!(
            std::path::Path::new(stored).exists(),
            "{stored} must be recoverable"
        );
        let expected = if original.ends_with("x.arw") {
            b"raw-bytes".to_vec()
        } else {
            b"same-bytes".to_vec()
        };
        assert_eq!(std::fs::read(stored).unwrap(), expected, "bytes preserved");
    }

    // Index: no rows, no contents, no evidence remain.
    let rows: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    let contents: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM contents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(contents, 0);
}

#[test]
fn cache_entries_go_when_the_last_copy_goes() {
    let f = fixture("cache-gc");
    std::fs::write(f.root.join("solo.jpg"), b"solo-bytes").unwrap();
    scan(&f);
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();
    // Simulate derived cache entries.
    for path in [f.cache.thumb(&hash), f.cache.preview(&hash)] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"webp").unwrap();
    }

    delete_item(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        DeleteMode::Trash,
    )
    .unwrap();
    assert!(!f.cache.thumb(&hash).exists());
    assert!(!f.cache.preview(&hash).exists());
}

#[test]
fn cache_entries_survive_while_another_copy_remains() {
    // The complementary branch of "when the LAST copy goes". The original test
    // had a single copy, so `remaining` was always 0 and the false branch —
    // the one that decides whether a shared cache entry survives — never ran.
    let f = fixture("cache-gc-shared");
    for sub in ["a", "b"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("x.jpg"), b"same-bytes").unwrap();
    }
    scan(&f);
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();
    for path in [f.cache.thumb(&hash), f.cache.preview(&hash)] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"webp").unwrap();
    }

    // One copy is deleted OUTSIDE the app (the supported out-of-app change),
    // leaving a live sibling: the shared cache entry must stay.
    std::fs::remove_file(f.root.join("b").join("x.jpg")).unwrap();
    scanner::walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert!(f.cache.thumb(&hash).exists(), "a live copy still needs it");

    // Now the last LIVE copy goes. The missing sibling must not pin the
    // identity: contents and cache both go, or they leak for the life of the
    // index (startup_sweep only reclaims cache whose hash left contents).
    delete_item(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        DeleteMode::Trash,
    )
    .unwrap();
    let contents: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM contents WHERE hash = ?1",
            rusqlite::params![hash],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(contents, 0, "a missing sibling must not pin the identity");
    assert!(!f.cache.thumb(&hash).exists());
    assert!(!f.cache.preview(&hash).exists());
}

#[test]
fn permanent_delete_removes_without_trashing() {
    let f = fixture("permanent");
    std::fs::write(f.root.join("gone.jpg"), b"bytes").unwrap();
    scan(&f);
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();

    delete_item(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        DeleteMode::Permanent,
    )
    .unwrap();
    assert!(!f.root.join("gone.jpg").exists());
    // Nothing landed in any trash under the app root.
    assert!(!f.app_root.join("trash").exists());
}

#[test]
fn unhashed_other_files_delete_by_path_id() {
    let f = fixture("by-path");
    std::fs::write(f.root.join("unique.bin"), vec![9u8; 77]).unwrap();
    scan(&f);
    let path_id: i64 = f
        .conn
        .query_row("SELECT id FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();

    let outcome = delete_item(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::PathId(path_id),
        DeleteMode::Trash,
    )
    .unwrap();
    assert_eq!(outcome.deleted_files, 1);
    assert!(!f.root.join("unique.bin").exists());
}

#[test]
fn move_out_delivers_primary_and_companion_then_trashes_the_rest() {
    let f = fixture("moveout");
    for sub in ["a", "b"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("x.jpg"), b"same-bytes").unwrap();
        std::fs::write(f.root.join(sub).join("x.arw"), b"raw-bytes").unwrap();
    }
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'x.jpg' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let outcome = move_out(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        &dest,
        MoveOutMode::MoveTrashRest,
    )
    .unwrap();

    assert_eq!(outcome.exported, 2, "primary + one companion instance");
    assert!(outcome.conflicts.is_empty());
    assert_eq!(std::fs::read(dest.join("x.jpg")).unwrap(), b"same-bytes");
    assert_eq!(std::fs::read(dest.join("x.arw")).unwrap(), b"raw-bytes");
    // All four originals left their places (post-action trashed them).
    assert_eq!(outcome.post_action.deleted_files, 4);
    assert!(!f.root.join("a").join("x.jpg").exists());
    assert!(!f.root.join("b").join("x.arw").exists());
    let rows: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "inbox-zero: nothing remains in the index");
}

#[test]
fn copy_mode_exports_and_leaves_everything_untouched() {
    let f = fixture("copy-mode");
    std::fs::write(f.root.join("keep.jpg"), b"kept-bytes").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();

    let outcome = move_out(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        &dest,
        MoveOutMode::CopyKeepAll,
    )
    .unwrap();
    assert_eq!(outcome.exported, 1);
    assert_eq!(outcome.post_action.deleted_files, 0);
    assert!(f.root.join("keep.jpg").exists(), "copy mode never deletes");
    assert!(dest.join("keep.jpg").exists());
}

#[test]
fn identical_destination_skips_but_still_runs_the_post_action() {
    let f = fixture("identical");
    std::fs::write(f.root.join("dup.jpg"), b"dup-bytes").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("dup.jpg"), b"dup-bytes").unwrap(); // already delivered
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();

    let outcome = move_out(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        &dest,
        MoveOutMode::MoveTrashRest,
    )
    .unwrap();
    assert_eq!(outcome.skipped_identical, 1);
    assert_eq!(outcome.exported, 0);
    assert_eq!(outcome.post_action.deleted_files, 1, "post-action proceeds");
    assert!(!f.root.join("dup.jpg").exists());
}

#[test]
fn conflicting_destination_blocks_and_withholds_the_post_action() {
    let f = fixture("conflict");
    std::fs::write(f.root.join("clash.jpg"), b"mine").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("clash.jpg"), b"theirs - different").unwrap();
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();

    let outcome = move_out(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        &dest,
        MoveOutMode::MoveTrashRest,
    )
    .unwrap();
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(outcome.exported, 0);
    assert_eq!(outcome.post_action.deleted_files, 0, "no destructive follow-up");
    assert!(f.root.join("clash.jpg").exists(), "originals untouched");
    assert_eq!(
        std::fs::read(dest.join("clash.jpg")).unwrap(),
        b"theirs - different".as_slice(),
        "the conflicting file is never overwritten"
    );
}

#[test]
fn a_rotted_copy_is_skipped_and_the_next_copy_delivers() {
    let f = fixture("rot");
    for sub in ["a", "b"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("r.jpg"), b"healthy-bytes").unwrap();
    }
    scan(&f);
    let hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'r.jpg' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Rot copy a AFTER indexing: same length, different bytes.
    std::fs::write(f.root.join("a").join("r.jpg"), b"rotten!-bytes").unwrap();

    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let outcome = move_out(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        &dest,
        MoveOutMode::CopyKeepAll,
    )
    .unwrap();

    assert_eq!(outcome.exported, 1);
    // The delivered bytes are the healthy ones, never the rotted ones.
    assert_eq!(std::fs::read(dest.join("r.jpg")).unwrap(), b"healthy-bytes");
    let issues: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM issues WHERE kind = 'copy-verify-mismatch'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(issues, 1, "the rotted copy is surfaced");
}

#[test]
fn a_failed_copy_keeps_its_row_and_records_an_issue() {
    let f = fixture("partial");
    std::fs::write(f.root.join("ok.jpg"), b"same").unwrap();
    std::fs::create_dir_all(f.root.join("b")).unwrap();
    std::fs::write(f.root.join("b").join("ok.jpg"), b"same").unwrap();
    scan(&f);
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();
    // Sabotage one copy: replace it with a directory so rename/remove fails.
    std::fs::remove_file(f.root.join("b").join("ok.jpg")).unwrap();
    std::fs::create_dir_all(f.root.join("b").join("ok.jpg")).unwrap();

    let outcome = delete_item(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        DeleteMode::Trash,
    )
    .unwrap();
    assert_eq!(outcome.deleted_files, 1);
    assert_eq!(outcome.failed_files, 1);

    // The failed copy's row survives; the contents row survives with it.
    let rows: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    let issues: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM issues WHERE kind = 'delete-error'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(issues, 1);
}

// A companion that does not reach the destination must withhold the
// post-action. Companions are RAW files and sidecars, never their own grid
// row, so deleting the source of one that was never delivered destroys it
// with nothing in the UI to reveal the loss.

#[test]
fn companion_conflict_withholds_the_post_action() {
    let f = fixture("companion-conflict");
    std::fs::write(f.root.join("x.jpg"), b"primary-bytes").unwrap();
    std::fs::write(f.root.join("x.arw"), b"raw-bytes").unwrap();
    scan(&f);

    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    // The RAW is already there with DIFFERENT content — a conflict.
    std::fs::write(dest.join("x.arw"), b"a-different-raw").unwrap();

    let hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'x.jpg'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let outcome = move_out(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        &dest,
        MoveOutMode::MoveTrashRest,
    )
    .unwrap();

    assert!(
        outcome.conflicts.iter().any(|c| c.ends_with("x.arw")),
        "the companion conflict must be reported, got {:?}",
        outcome.conflicts
    );
    assert_eq!(
        outcome.post_action.deleted_files, 0,
        "nothing may be deleted while a companion is undelivered"
    );
    assert!(f.root.join("x.jpg").exists(), "primary must survive");
    assert!(f.root.join("x.arw").exists(), "the RAW must survive");
}

#[test]
fn companion_copy_failure_withholds_the_post_action() {
    let f = fixture("companion-copy-fail");
    std::fs::write(f.root.join("x.jpg"), b"primary-bytes").unwrap();
    std::fs::write(f.root.join("x.arw"), b"raw-bytes").unwrap();
    scan(&f);

    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();

    // Make the source RAW unreadable so every copy attempt fails. This is the
    // ENOSPC shape: deliver_one returns not-ok having pushed NO conflict, so
    // it is the failure the outcome could not previously express at all.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            f.root.join("x.arw"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
    }

    let hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'x.jpg'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let outcome = move_out(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        &dest,
        MoveOutMode::MoveTrashRest,
    )
    .unwrap();

    #[cfg(unix)]
    {
        assert_eq!(
            outcome.post_action.deleted_files, 0,
            "nothing may be deleted while a companion could not be copied"
        );
        assert!(
            !outcome.undelivered.is_empty(),
            "an undeliverable companion must be reported, not silently dropped"
        );
        assert!(f.root.join("x.jpg").exists(), "primary must survive");
        assert!(f.root.join("x.arw").exists(), "the RAW must survive");
    }
}

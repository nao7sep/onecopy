// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::extensions;
use onecopy_lib::index_store;
use onecopy_lib::operations::*;
use onecopy_lib::preview::CachePaths;
use onecopy_lib::scanner::{self, ScanLists};
use rusqlite::Connection;

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
        audio: owned(extensions::AUDIO_EXTENSIONS),
        companions: owned(extensions::COMPANION_EXTENSIONS),
    }
}

fn item_projection() -> onecopy_lib::queries::ItemProjectionContext {
    onecopy_lib::queries::ItemProjectionContext {
        capabilities: onecopy_lib::derived_state::WorkCapabilities {
            ffmpeg: true,
            video_snapshots_enabled: true,
            similarity_enabled: true,
            face_enabled: false,
            face_models: false,
            transcription_model: false,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
        },
        similarity_dirty: false,
    }
}

fn scan(f: &Fixture) {
    scanner::walk_root(&f.conn, &f.root, &lists()).unwrap();
    scanner::hash_pending(&f.conn, &f.cache).unwrap();
    scanner::pair_companions(&f.conn, true).unwrap();
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
    let manifest: Vec<serde_json::Value> = std::fs::read_to_string(day_dir.join("manifest.jsonl"))
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
fn delete_batch_plans_once_and_cancels_between_physical_files() {
    let f = fixture("delete-batch-cancel");
    std::fs::write(f.root.join("a.jpg"), vec![1u8; 10]).unwrap();
    std::fs::write(f.root.join("a.xmp"), vec![2u8; 20]).unwrap();
    std::fs::write(f.root.join("b.jpg"), vec![3u8; 30]).unwrap();
    scan(&f);
    let hash = |name: &str| -> String {
        f.conn
            .query_row(
                "SELECT content_hash FROM paths WHERE file_name = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    };
    let a = hash("a.jpg");
    let b = hash("b.jpg");
    let cancel = std::cell::Cell::new(false);
    let progress = std::cell::RefCell::new(Vec::new());

    let outcome = delete_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[
            ItemIdentity {
                hash: Some(a.clone()),
                path_id: None,
            },
            ItemIdentity {
                hash: Some(a),
                path_id: None,
            },
            ItemIdentity {
                hash: Some(b),
                path_id: None,
            },
        ],
        DeleteMode::Permanent,
        &|| cancel.get(),
        |snapshot| {
            if matches!(
                snapshot,
                DeleteBatchProgress::Deleting { files_done: 1, .. }
            ) {
                cancel.set(true);
            }
            progress.borrow_mut().push(snapshot);
        },
    )
    .unwrap();

    assert!(outcome.cancelled);
    assert!(outcome.items.is_empty(), "the interrupted logical item is partial");
    assert_eq!(
        outcome.files_total, 3,
        "primary, companion, and second item"
    );
    assert_eq!(outcome.bytes_total, 60);
    assert_eq!(outcome.deleted_files, 1);
    assert!(f.root.join("a.jpg").exists(), "the next physical step is not started");
    assert!(!f.root.join("a.xmp").exists());
    assert!(
        f.root.join("b.jpg").exists(),
        "the unstarted unit is untouched"
    );
    assert!(progress.borrow().iter().any(|snapshot| matches!(
        snapshot,
        DeleteBatchProgress::Planning {
            items_done: 2,
            items_total: 2,
            ..
        }
    )));
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
    let (path_id, hash): (i64, Option<String>) = f
        .conn
        .query_row("SELECT id, content_hash FROM paths LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    // The premise the name rests on: an other-file with a unique size is never
    // read, so it carries no hash and can only be addressed by path id.
    assert_eq!(hash, None, "a unique-size other-file stays unhashed");

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

    // The index must forget it, and the file must be recoverable.
    let rows: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "the path row is removed, not left behind");
    let day_dir = std::fs::read_dir(f.app_root.join("trash"))
        .expect("the trash root exists")
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("one day folder");
    let manifest = std::fs::read_to_string(day_dir.join("manifest.jsonl")).unwrap();
    let line: serde_json::Value = serde_json::from_str(manifest.lines().next().unwrap()).unwrap();
    let stored = line["storedPath"].as_str().unwrap();
    assert_eq!(std::fs::read(stored).unwrap(), vec![9u8; 77]);
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
    // All four originals left their places (post-action trashed them). The
    // counter alone is a value the code under test produced; what matters is
    // that the files are actually gone AND actually recoverable.
    assert_eq!(outcome.post_action.deleted_files, 4);
    for original in [
        f.root.join("a").join("x.jpg"),
        f.root.join("b").join("x.jpg"),
        f.root.join("a").join("x.arw"),
        f.root.join("b").join("x.arw"),
    ] {
        assert!(!original.exists(), "{} must be gone", original.display());
    }
    let day_dir = std::fs::read_dir(f.app_root.join("trash"))
        .expect("the trash root exists")
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("one day folder");
    let manifest: Vec<serde_json::Value> = std::fs::read_to_string(day_dir.join("manifest.jsonl"))
        .expect("a manifest was written")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        manifest.len(),
        4,
        "every trashed original has a manifest line"
    );
    for line in &manifest {
        let stored = line["storedPath"].as_str().expect("storedPath");
        assert!(
            std::path::Path::new(stored).exists(),
            "{stored} must be recoverable"
        );
    }
    assert!(!f.root.join("a").join("x.jpg").exists());
    assert!(!f.root.join("b").join("x.arw").exists());
    let rows: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "inbox-zero: nothing remains in the index");
}

#[test]
fn companion_name_collisions_follow_the_destination_filesystem_and_representative_priority() {
    let f = fixture("natural-companion-case");
    for sub in ["a", "b"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("x.jpg"), b"same-primary").unwrap();
    }
    std::fs::write(f.root.join("a").join("x.xmp"), b"representative-sidecar").unwrap();
    std::fs::write(f.root.join("b").join("x.XMP"), b"later-sidecar").unwrap();
    scan(&f);
    f.conn
        .execute(
            "UPDATE paths SET resolved_utc_ms = CASE dir_path WHEN ?1 THEN 1000 ELSE 2000 END \
             WHERE file_name = 'x.jpg'",
            rusqlite::params![f.root.join("a").to_string_lossy()],
        )
        .unwrap();

    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let lower_probe = dest.join("case-probe");
    let upper_probe = dest.join("CASE-PROBE");
    std::fs::write(&lower_probe, b"probe").unwrap();
    let case_sensitive = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&upper_probe)
        .is_ok();
    std::fs::remove_file(&lower_probe).unwrap();
    if case_sensitive {
        std::fs::remove_file(&upper_probe).unwrap();
    }

    let hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'x.jpg' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let outcome = move_out(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        &dest,
        MoveOutMode::MoveDeleteRest,
    )
    .unwrap();

    assert_eq!(outcome.post_action.deleted_files, 4);
    assert_eq!(std::fs::read(dest.join("x.xmp")).unwrap(), b"representative-sidecar");
    if case_sensitive {
        assert_eq!(std::fs::read(dest.join("x.XMP")).unwrap(), b"later-sidecar");
        assert_eq!(outcome.exported, 3);
    } else {
        assert_eq!(outcome.exported, 2, "the natural collision publishes one sidecar");
    }
}

#[test]
fn destination_batch_preflights_internal_collisions_and_renames_the_complete_set() {
    let f = fixture("batch-name-collision");
    for (dir, bytes) in [("a", b"first".as_slice()), ("b", b"second".as_slice())] {
        std::fs::create_dir_all(f.root.join(dir)).unwrap();
        std::fs::write(f.root.join(dir).join("same.jpg"), bytes).unwrap();
    }
    scan(&f);
    let mut stmt = f
        .conn
        .prepare("SELECT DISTINCT content_hash FROM paths ORDER BY content_hash")
        .unwrap();
    let items = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|hash| ItemIdentity {
            hash: Some(hash.unwrap()),
            path_id: None,
        })
        .collect::<Vec<_>>();
    drop(stmt);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();

    let review = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &items,
        &dest,
        MoveOutMode::CopyKeepAll,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert!(review.requires_conflict_choice);
    assert!(!review.overwrite_allowed);
    assert!(review.items.is_empty());
    assert_eq!(review.reviewed_conflicts.len(), 1);
    assert!(review.reviewed_conflicts[0].within_selection);
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0);

    let token = review.plan_token.as_deref().expect("review token");
    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        &items,
        &dest,
        MoveOutMode::CopyKeepAll,
        Some(DestinationConflictPolicy::Rename),
        Some(token),
        DestinationRenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert_eq!(outcome.items.len(), 2);
    assert_eq!(outcome.exported, 2);
    assert!(outcome.conflicts.is_empty());
    let mut delivered = [
        std::fs::read(dest.join("same.jpg")).unwrap(),
        std::fs::read(dest.join("same 2.jpg")).unwrap(),
    ];
    delivered.sort();
    assert_eq!(delivered, [b"first".to_vec(), b"second".to_vec()]);
}

#[test]
fn cancellation_during_private_streaming_publishes_nothing() {
    let f = fixture("batch-private-cancel");
    std::fs::write(f.root.join("large.jpg"), vec![7u8; 2 * 1024 * 1024]).unwrap();
    scan(&f);
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let item = ItemIdentity {
        hash: Some(hash),
        path_id: None,
    };
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let stop = std::cell::Cell::new(false);

    let outcome = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item],
        &dest,
        MoveOutMode::MoveDeleteRest,
        &|| stop.get(),
        |progress| {
            if matches!(
                progress,
                MoveBatchProgress::Delivering {
                    current_file_bytes_done: Some(0),
                    ..
                }
            ) {
                stop.set(true);
            }
        },
    )
    .unwrap();

    assert!(outcome.cancelled);
    assert!(
        outcome.items.is_empty(),
        "the private unit remains unstarted"
    );
    assert!(f.root.join("large.jpg").exists());
    assert!(!dest.join("large.jpg").exists());
    assert_eq!(
        std::fs::read_dir(&dest).unwrap().count(),
        0,
        "private stage cleaned"
    );
}

#[test]
fn cancellation_after_publication_stops_before_the_next_physical_source_action() {
    let f = fixture("batch-commit-boundary");
    for name in ["a.jpg", "b.jpg"] {
        std::fs::write(f.root.join(name), name.as_bytes()).unwrap();
    }
    scan(&f);
    let mut stmt = f
        .conn
        .prepare("SELECT content_hash FROM paths ORDER BY file_name")
        .unwrap();
    let items = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|hash| ItemIdentity {
            hash: Some(hash.unwrap()),
            path_id: None,
        })
        .collect::<Vec<_>>();
    drop(stmt);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let stop = std::cell::Cell::new(false);

    let outcome = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &items,
        &dest,
        MoveOutMode::MoveDeleteRest,
        &|| stop.get(),
        |progress| {
            if matches!(
                progress,
                MoveBatchProgress::Delivering { files_done: 1, .. }
            ) {
                stop.set(true);
            }
        },
    )
    .unwrap();

    assert!(outcome.cancelled);
    assert_eq!(outcome.items.len(), 1, "the published partial result is reported");
    assert!(dest.join("a.jpg").exists());
    assert!(
        f.root.join("a.jpg").exists(),
        "cancellation takes effect before source cleanup"
    );
    assert!(!dest.join("b.jpg").exists());
    assert!(f.root.join("b.jpg").exists(), "next unit stayed untouched");
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
fn conflicting_destination_waits_for_one_reviewed_policy_before_any_effect() {
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

    let item = ItemIdentity {
        hash: Some(hash),
        path_id: None,
    };
    let review = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        std::slice::from_ref(&item),
        &dest,
        MoveOutMode::MoveTrashRest,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert!(review.requires_conflict_choice);
    assert_eq!(review.reviewed_conflicts.len(), 1);
    assert!(review.items.is_empty());
    assert!(f.root.join("clash.jpg").exists(), "originals untouched");
    assert_eq!(
        std::fs::read(dest.join("clash.jpg")).unwrap(),
        b"theirs - different".as_slice(),
        "the conflicting file is never overwritten"
    );

    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item],
        &dest,
        MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Rename),
        review.plan_token.as_deref(),
        DestinationRenameStyle::ParenthesizedNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.exported, 1);
    assert_eq!(outcome.post_action.deleted_files, 1);
    assert_eq!(std::fs::read(dest.join("clash (2).jpg")).unwrap(), b"mine");
    assert!(!f.root.join("clash.jpg").exists());
}

#[test]
fn overwrite_preserves_the_reviewed_destination_family_before_publication() {
    let f = fixture("overwrite-family");
    std::fs::write(f.root.join("x.jpg"), b"new-primary").unwrap();
    std::fs::write(f.root.join("x.xmp"), b"new-sidecar").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("x.jpg"), b"old-primary").unwrap();
    std::fs::write(dest.join("x.xmp"), b"old-sidecar").unwrap();
    let hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'x.jpg'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let item = ItemIdentity {
        hash: Some(hash),
        path_id: None,
    };

    let review = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        std::slice::from_ref(&item),
        &dest,
        MoveOutMode::MoveTrashRest,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert!(review.requires_conflict_choice);
    assert!(review.overwrite_allowed);
    assert!(review
        .reviewed_conflicts
        .iter()
        .any(|conflict| conflict.preserved_paths.len() == 2));

    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item],
        &dest,
        MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Overwrite),
        review.plan_token.as_deref(),
        DestinationRenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.exported, 2);
    assert_eq!(outcome.post_action.deleted_files, 2);
    assert_eq!(std::fs::read(dest.join("x.jpg")).unwrap(), b"new-primary");
    assert_eq!(std::fs::read(dest.join("x.xmp")).unwrap(), b"new-sidecar");

    let day_dir = std::fs::read_dir(f.app_root.join("trash"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .expect("destination replacements are recoverable");
    let originals = std::fs::read_to_string(day_dir.join("manifest.jsonl"))
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["originalPath"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(originals.contains(&dest.join("x.jpg").to_string_lossy().into_owned()));
    assert!(originals.contains(&dest.join("x.xmp").to_string_lossy().into_owned()));
}

#[test]
fn changed_destination_review_refuses_overwrite_without_filesystem_effects() {
    let f = fixture("overwrite-review-change");
    std::fs::write(f.root.join("x.jpg"), b"source-bytes").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("x.jpg"), b"old-version1").unwrap();
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |row| row.get(0))
        .unwrap();
    let item = ItemIdentity {
        hash: Some(hash),
        path_id: None,
    };
    let review = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        std::slice::from_ref(&item),
        &dest,
        MoveOutMode::MoveTrashRest,
        &|| false,
        |_| {},
    )
    .unwrap();
    std::fs::write(dest.join("x.jpg"), b"old-version2").unwrap();

    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item],
        &dest,
        MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Overwrite),
        review.plan_token.as_deref(),
        DestinationRenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert!(outcome.plan_changed);
    assert!(outcome.items.is_empty());
    assert!(f.root.join("x.jpg").exists());
    assert_eq!(std::fs::read(dest.join("x.jpg")).unwrap(), b"old-version2");
    assert!(!f.app_root.join("trash").exists());
}

#[test]
fn a_changed_copy_is_delivered_as_it_exists_when_the_operation_runs() {
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
    // Change representative copy a after indexing: same length, different bytes.
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
    assert_eq!(std::fs::read(dest.join("r.jpg")).unwrap(), b"rotten!-bytes");
    let issues: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))
        .unwrap();
    assert_eq!(issues, 0, "the operation does not enforce the older indexed bytes");
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

#[test]
fn companion_conflict_renames_the_complete_output_family_consistently() {
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

    let item = ItemIdentity {
        hash: Some(hash),
        path_id: None,
    };
    let review = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        std::slice::from_ref(&item),
        &dest,
        MoveOutMode::MoveTrashRest,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert!(review.requires_conflict_choice);
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 1);
    assert!(f.root.join("x.jpg").exists());
    assert!(f.root.join("x.arw").exists());

    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item],
        &dest,
        MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Rename),
        review.plan_token.as_deref(),
        DestinationRenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.exported, 2);
    assert_eq!(outcome.post_action.deleted_files, 2);
    assert_eq!(std::fs::read(dest.join("x 2.jpg")).unwrap(), b"primary-bytes");
    assert_eq!(std::fs::read(dest.join("x 2.arw")).unwrap(), b"raw-bytes");
    assert_eq!(std::fs::read(dest.join("x.arw")).unwrap(), b"a-different-raw");
}

// Unix-only, gated at the ITEM so Windows is honestly MISSING this coverage
// rather than running it vacuously green: the failure is staged with a chmod
// 0o000 that Windows has no equivalent for, and every assertion below depends
// on that staging.
#[cfg(unix)]
#[test]
fn companion_copy_failure_preserves_that_companion_without_rolling_back_the_primary() {
    let f = fixture("companion-copy-fail");
    std::fs::write(f.root.join("x.jpg"), b"primary-bytes").unwrap();
    std::fs::write(f.root.join("x.arw"), b"raw-bytes").unwrap();
    scan(&f);

    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();

    // Make the source RAW unreadable so its output alone cannot be staged.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(f.root.join("x.arw"), std::fs::Permissions::from_mode(0o000))
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

    assert_eq!(outcome.post_action.deleted_files, 1);
    assert!(
        !outcome.undelivered.is_empty(),
        "an undeliverable companion must be reported, not silently dropped"
    );
    assert!(
        dest.join("x.jpg").exists(),
        "the verified primary remains published"
    );
    assert!(!f.root.join("x.jpg").exists(), "the delivered primary is cleaned");
    assert!(f.root.join("x.arw").exists(), "the RAW must survive");
}

#[cfg(unix)]
#[test]
fn destination_write_failure_stops_the_unstarted_remainder_and_records_an_issue() {
    let f = fixture("destination-write-failure");
    for name in ["a.jpg", "b.jpg"] {
        std::fs::write(f.root.join(name), name.as_bytes()).unwrap();
    }
    scan(&f);
    let mut statement = f
        .conn
        .prepare("SELECT content_hash FROM paths ORDER BY file_name")
        .unwrap();
    let items = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|hash| ItemIdentity {
            hash: Some(hash.unwrap()),
            path_id: None,
        })
        .collect::<Vec<_>>();
    drop(statement);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o500)).unwrap();
    }

    let outcome = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &items,
        &dest,
        MoveOutMode::MoveTrashRest,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert!(outcome.error.is_some());
    assert!(outcome.items.is_empty());
    assert!(f.root.join("a.jpg").exists());
    assert!(f.root.join("b.jpg").exists());
    let issues: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM issues WHERE kind = 'copy-error'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(issues, 1);
}

#[test]
fn shift_move_out_permanently_deletes_the_remaining_copies() {
    // MoveDeleteRest is the only mode that destroys files with NO recovery —
    // no trash, no undo — and it had zero tests.
    let f = fixture("move-delete-rest");
    for sub in ["a", "b"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("x.jpg"), b"same-bytes").unwrap();
    }
    std::fs::write(f.root.join("a").join("x.arw"), b"raw-bytes").unwrap();
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
        MoveOutMode::MoveDeleteRest,
    )
    .unwrap();

    assert_eq!(outcome.exported, 2, "primary + one companion instance");
    assert!(outcome.conflicts.is_empty());
    assert!(outcome.undelivered.is_empty());
    assert_eq!(std::fs::read(dest.join("x.jpg")).unwrap(), b"same-bytes");
    assert_eq!(std::fs::read(dest.join("x.arw")).unwrap(), b"raw-bytes");

    // Every original is gone from disk...
    for original in [
        f.root.join("a").join("x.jpg"),
        f.root.join("b").join("x.jpg"),
        f.root.join("a").join("x.arw"),
    ] {
        assert!(!original.exists(), "{} must be gone", original.display());
    }
    // ...the index forgot them...
    let rows: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    // ...and NOTHING was trashed. That is the whole difference between this
    // mode and MoveTrashRest, and the reason it needs a confirmation.
    assert!(
        !f.app_root.join("trash").exists(),
        "MoveDeleteRest must not write a trash — it is the no-recovery mode"
    );
}

#[test]
fn copy_count_matches_the_rows_a_delete_targets() {
    // The badge doubles as a backup health check, so it must describe the same
    // set the delete destroys. section_items counts only rows with
    // companion_of IS NULL; delete_item takes every row sharing the hash.
    let f = fixture("count-vs-delete");
    for sub in ["a", "b"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("x.jpg"), b"same-bytes").unwrap();
    }
    scan(&f);

    let hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'x.jpg' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let items = onecopy_lib::queries::section_items(
        &f.conn,
        "image",
        "undated",
        chrono_tz::Tz::UTC,
        item_projection(),
    )
    .unwrap();
    let shown = items
        .iter()
        .find(|i| i.hash.as_deref() == Some(hash.as_str()))
        .expect("the logical item is in its section");
    let badge = shown.copy_count;

    let outcome = delete_item(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        DeleteMode::Trash,
    )
    .unwrap();

    assert_eq!(
        outcome.removed_rows, badge,
        "the badge must describe exactly the rows a delete destroys"
    );
}

#[test]
fn a_shared_hash_split_across_paired_and_unpaired_rows_still_agrees() {
    // The divergence the plain case cannot reach: one content hash held by BOTH
    // a companion row (excluded from the badge, which filters companion_of IS
    // NULL) and a standalone row (counted). dir a has a JPEG so its ARW pairs;
    // dir b has the identical ARW with no JPEG beside it, so it stands alone as
    // an other-file. A delete takes every row sharing the hash.
    let f = fixture("count-split");
    for sub in ["a", "b"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("x.arw"), b"raw-bytes").unwrap();
    }
    std::fs::write(f.root.join("a").join("x.jpg"), b"jpeg-bytes").unwrap();
    scan(&f);

    let raw_hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'x.arw' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let paired: i64 = f
        .conn
        .query_row(
            "SELECT COUNT(*) FROM paths WHERE content_hash = ?1 AND companion_of IS NOT NULL",
            rusqlite::params![raw_hash],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(paired, 1, "exactly one of the two ARWs pairs");

    let items = onecopy_lib::queries::section_items(
        &f.conn,
        "other",
        "undated",
        chrono_tz::Tz::UTC,
        item_projection(),
    )
    .unwrap();
    let badge = items
        .iter()
        .find(|i| i.hash.as_deref() == Some(raw_hash.as_str()))
        .expect("the unpaired ARW is an other-file")
        .copy_count;

    let outcome = delete_item(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&raw_hash),
        DeleteMode::Trash,
    )
    .unwrap();

    assert_eq!(
        outcome.removed_rows, badge,
        "the badge under-reports what the delete destroys when a companion \
         shares the hash"
    );
}

#[test]
fn unhashed_other_files_move_out_and_conflict_correctly_by_path_id() {
    // Every other move_out test uses ItemRef::Hash. The PathId path skips tee
    // verification entirely — there is no indexed hash to verify against — and
    // instead compares the destination against the first copy's bytes re-read
    // from disk, so it is a genuinely different code path.
    let cases: [(&str, &[u8], u64, u64, bool); 3] = [
        // (label, pre-existing destination bytes, exported, skipped, needs review)
        ("empty", b"", 1, 0, false),
        ("identical", b"unique-payload", 0, 1, false),
        ("different", b"something-else", 0, 0, true),
    ];
    for (label, existing, exported, skipped, needs_review) in cases {
        let f = fixture(&format!("moveout-pathid-{label}"));
        std::fs::write(f.root.join("unique.bin"), b"unique-payload").unwrap();
        scan(&f);
        let path_id: i64 = f
            .conn
            .query_row("SELECT id FROM paths LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let dest = f._dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        if !existing.is_empty() {
            std::fs::write(dest.join("unique.bin"), existing).unwrap();
        }

        let item = ItemIdentity {
            hash: None,
            path_id: Some(path_id),
        };
        let outcome = move_batch(
            &f.conn,
            &f.app_root,
            &f.cache,
            std::slice::from_ref(&item),
            &dest,
            MoveOutMode::MoveTrashRest,
            &|| false,
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome.exported, exported, "{label}: exported");
        assert_eq!(outcome.skipped_identical, skipped, "{label}: skipped");
        assert_eq!(
            outcome.requires_conflict_choice, needs_review,
            "{label}: review"
        );
        assert_eq!(
            std::fs::read(dest.join("unique.bin")).unwrap(),
            if existing.is_empty() {
                b"unique-payload".to_vec()
            } else {
                existing.to_vec()
            },
            "{label}: the destination holds what it should"
        );

        if !needs_review {
            // Delivered (or already there): the post-action ran.
            assert!(
                !f.root.join("unique.bin").exists(),
                "{label}: the original was handled"
            );
        } else {
            // The complete conflict review has no filesystem effects.
            assert_eq!(outcome.post_action.deleted_files, 0, "{label}: no delete");
            assert_eq!(outcome.reviewed_conflicts.len(), 1);
            assert!(
                f.root.join("unique.bin").exists(),
                "{label}: a conflicting move must leave the original alone"
            );
        }
    }
}

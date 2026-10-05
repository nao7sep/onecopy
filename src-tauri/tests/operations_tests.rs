// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::extensions;
use onecopy_lib::index_store;
use onecopy_lib::file_names::{FolderNames, RenameStyle};
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
    std::fs::write(
        app_root.join("config.json"),
        serde_json::to_vec(&serde_json::json!({
            "sourceDirs": [root.to_string_lossy()],
            "destinationRoots": [dir.path().to_string_lossy()],
        }))
        .unwrap(),
    )
    .unwrap();
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
            transcription_acceleration: true,
            face_acceleration: true,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
        },
    }
}

fn section_items(f: &Fixture, kind: &str) -> Vec<onecopy_lib::queries::SectionItem> {
    onecopy_lib::queries::section_window(
        &f.conn,
        serde_json::from_value(serde_json::json!(kind)).expect("a section kind"),
        "undated",
        chrono_tz::Tz::UTC,
        onecopy_lib::queries::SectionSort {
            order: onecopy_lib::queries::SectionSortOrder::Time,
            desc: false,
        },
        0,
        onecopy_lib::queries::MAX_SECTION_WINDOW_ITEMS,
        item_projection(),
    )
    .unwrap()
    .items
}

fn scan(f: &Fixture) {
    scanner::walk_root(&f.conn, &f.root, &lists()).unwrap();
    scanner::hash_pending(&f.conn, &f.cache).unwrap();
    scanner::pair_companions(&f.conn, true).unwrap();
}

#[test]
fn visibility_never_exempts_identical_hidden_copies_from_cleanup() {
    for action in ["copy", "move", "trash", "permanent"] {
        let f = fixture("hidden-copy-cleanup");
        for name in ["photo.jpg", ".photo.jpg"] {
            std::fs::write(f.root.join(name), b"same-image-bytes").unwrap();
        }
        std::fs::write(f.root.join(".different.jpg"), b"different-image").unwrap();
        scan(&f);
        let hash: String = f.conn.query_row("SELECT content_hash FROM paths WHERE file_name = 'photo.jpg'", [], |row| row.get(0)).unwrap();
        let item = ItemIdentity { hash: Some(hash.clone()), path_id: None };
        let detail = onecopy_lib::queries::item_detail(&f.conn, Some(&hash), None).unwrap();
        assert_eq!(detail.file_name, "photo.jpg");
        assert_eq!(detail.copy_paths.len(), 2);
        if matches!(action, "copy" | "move") {
            let dest = f._dir.path().join("destination");
            std::fs::create_dir(&dest).unwrap();
            let outcome = move_batch(&f.conn, &f.app_root, &f.cache, &[item], &dest,
                if action == "copy" { MoveOutMode::CopyKeepAll } else { MoveOutMode::MoveTrashRest }, &|| false, |_| {}).unwrap();
            assert_eq!(outcome.exported, 1);
            assert_eq!(std::fs::read(dest.join("photo.jpg")).unwrap(), b"same-image-bytes");
            assert!(!dest.join(".photo.jpg").exists());
            assert_eq!(outcome.post_action.deleted_files, if action == "copy" { 0 } else { 2 });
        } else {
            let outcome = delete_item(&f.conn, &f.app_root, &f.cache, ItemRef::Hash(&hash),
                if action == "trash" { DeleteMode::Trash } else { DeleteMode::Permanent }).unwrap();
            assert_eq!(outcome.deleted_files, 2);
            assert_eq!(outcome.failed_files, 0);
        }
        for name in ["photo.jpg", ".photo.jpg"] { assert_eq!(f.root.join(name).exists(), action == "copy", "{action}: {name}"); }
        assert_eq!(std::fs::read(f.root.join(".different.jpg")).unwrap(), b"different-image");
    }
}

#[test]
fn visibility_changes_before_conflict_acceptance_require_fresh_review() {
    let f = fixture("visibility-frozen-delivery");
    for name in ["photo.jpg", ".photo.jpg"] { std::fs::write(f.root.join(name), b"same-image").unwrap(); }
    scan(&f);
    let hash: String = f.conn.query_row("SELECT content_hash FROM paths LIMIT 1", [], |row| row.get(0)).unwrap();
    let items = [ItemIdentity { hash: Some(hash), path_id: None }];
    let dest = f._dir.path().join("destination");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("photo.jpg"), b"old-image").unwrap();
    let review = move_batch(&f.conn, &f.app_root, &f.cache, &items, &dest, MoveOutMode::MoveTrashRest, &|| false, |_| {}).unwrap();
    assert!(review.requires_conflict_choice);
    onecopy_lib::visibility_index::apply_policy(&f.conn,
        &onecopy_lib::visibility::Policy::from_config(&serde_json::json!({"hideDotNames": false})).unwrap()).unwrap();
    std::fs::write(f.root.join("late.jpg"), b"same-image").unwrap();
    scan(&f);
    let result = move_batch_reviewed(&f.conn, &f.app_root, &f.cache, &items, &AcceptedFiles::capture(&f.conn, &items).unwrap(), &dest, MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Overwrite), review.plan_token.as_deref(), RenameStyle::SpaceNumber, &|| false, |_| {}).unwrap();
    assert!(result.plan_changed);
    assert_eq!(std::fs::read(dest.join("photo.jpg")).unwrap(), b"old-image");
    assert!(!dest.join(".photo.jpg").exists());
    assert_eq!(result.post_action.deleted_files, 0);
    assert!(f.root.join("photo.jpg").exists());
    assert!(f.root.join(".photo.jpg").exists());
    assert!(f.root.join("late.jpg").exists());
}

#[test]
fn visibility_changes_do_not_rename_or_broaden_an_accepted_delivery() {
    let f = fixture("visibility-accepted-delivery");
    for name in ["photo.jpg", ".photo.jpg"] { std::fs::write(f.root.join(name), b"same-image").unwrap(); }
    scan(&f);
    let hash: String = f.conn.query_row("SELECT content_hash FROM paths LIMIT 1", [], |row| row.get(0)).unwrap();
    let items = [ItemIdentity { hash: Some(hash), path_id: None }];
    let dest = f._dir.path().join("destination");
    std::fs::create_dir(&dest).unwrap();
    let mut changed = false;
    let result = move_batch(&f.conn, &f.app_root, &f.cache, &items, &dest, MoveOutMode::MoveTrashRest, &|| false, |progress| {
        if changed || !matches!(progress, MoveBatchProgress::Delivering { .. }) { return; }
        changed = true;
        onecopy_lib::visibility_index::apply_policy(&f.conn,
            &onecopy_lib::visibility::Policy::from_config(&serde_json::json!({"hideDotNames": false})).unwrap()).unwrap();
        std::fs::write(f.root.join("late.jpg"), b"same-image").unwrap();
        scan(&f);
    }).unwrap();
    assert!(changed);
    assert_eq!(std::fs::read(dest.join("photo.jpg")).unwrap(), b"same-image");
    assert!(!dest.join(".photo.jpg").exists());
    assert_eq!(result.post_action.deleted_files, 2);
    assert!(f.root.join("late.jpg").exists());
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

    let day_dir = std::fs::read_dir(f.root.join(onecopy_lib::trash::TRASH_DIR_NAME))
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
    // Version 2: every line names its operation, item, role and relative
    // location, plus the size and time a restore verifies.
    let operation = manifest[0]["operation"].as_str().expect("operation id").to_string();
    for line in &manifest {
        let original = line["originalPath"].as_str().unwrap();
        assert_eq!(line["v"], 2);
        assert_eq!(line["kind"], "delete");
        assert_eq!(line["operation"], operation.as_str(), "one operation id per batch");
        assert_eq!(line["item"], hash.as_str());
        let companion = original.ends_with("x.arw");
        assert_eq!(line["role"], if companion { "companion" } else { "main" });
        let relative = line["originalRelative"].as_str().unwrap();
        assert!(["a/x.jpg", "b/x.jpg", "a/x.arw"].contains(&relative), "{relative}");
        assert!(original.ends_with(&relative.replace('/', std::path::MAIN_SEPARATOR_STR)));
        assert_eq!(
            line["storedName"].as_str().unwrap(),
            std::path::Path::new(line["storedPath"].as_str().unwrap())
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        let stored_meta = std::fs::metadata(line["storedPath"].as_str().unwrap()).unwrap();
        assert_eq!(line["size"], stored_meta.len());
        let mtime = stored_meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert_eq!(line["mtimeMs"], mtime, "a rename keeps the modification time");
    }
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
    assert!(
        outcome.items.is_empty(),
        "the interrupted logical item is partial"
    );
    assert_eq!(
        outcome.files_total, 3,
        "primary, companion, and second item"
    );
    assert_eq!(outcome.bytes_total, 60);
    assert_eq!(outcome.deleted_files, 1);
    assert!(
        f.root.join("a.jpg").exists(),
        "the next physical step is not started"
    );
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
    assert!(!f.root.join(onecopy_lib::trash::TRASH_DIR_NAME).exists());
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
    let day_dir = std::fs::read_dir(f.root.join(onecopy_lib::trash::TRASH_DIR_NAME))
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
    let day_dir = std::fs::read_dir(f.root.join(onecopy_lib::trash::TRASH_DIR_NAME))
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
        assert_eq!(line["v"], 2);
        assert_eq!(line["kind"], "move-cleanup");
        assert_eq!(line["item"], hash.as_str());
        let companion = line["originalPath"].as_str().unwrap().ends_with("x.arw");
        assert_eq!(line["role"], if companion { "companion" } else { "main" });
        let moved_to = line["movedTo"].as_str().expect("the output that replaced it");
        let expected = dest.join(if companion { "x.arw" } else { "x.jpg" });
        assert_eq!(moved_to, expected.to_string_lossy());
    }
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
    assert_eq!(
        std::fs::read(dest.join("x.xmp")).unwrap(),
        b"representative-sidecar"
    );
    if case_sensitive {
        assert_eq!(std::fs::read(dest.join("x.XMP")).unwrap(), b"later-sidecar");
        assert_eq!(outcome.exported, 3);
    } else {
        assert_eq!(
            outcome.exported, 2,
            "the natural collision publishes one sidecar"
        );
    }
    assert!(private_leftovers(&dest).is_empty(), "{:?}", private_leftovers(&dest));
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
        &AcceptedFiles::capture(&f.conn, &items).unwrap(),
        &dest,
        MoveOutMode::CopyKeepAll,
        Some(DestinationConflictPolicy::Rename),
        Some(token),
        RenameStyle::SpaceNumber,
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
    assert_eq!(
        outcome.items.len(),
        1,
        "the published partial result is reported"
    );
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
fn copy_and_move_outputs_keep_the_source_file_times() {
    let f = fixture("keep-times");
    let dated = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    for (name, bytes) in [("copied.jpg", b"copied-bytes"), ("moved.jpg", b"moved-bytes!")] {
        let path = f.root.join(name);
        std::fs::write(&path, bytes).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(dated)).unwrap();
    }
    scan(&f);
    let created = |path: &std::path::Path| std::fs::metadata(path).unwrap().created().ok();
    let copied_born = created(&f.root.join("copied.jpg"));
    let moved_born = created(&f.root.join("moved.jpg"));
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let hash_of = |name: &str| -> String {
        f.conn
            .query_row("SELECT content_hash FROM paths WHERE file_name = ?1", [name], |r| r.get(0))
            .unwrap()
    };

    for (name, mode) in [
        ("copied.jpg", MoveOutMode::CopyKeepAll),
        ("moved.jpg", MoveOutMode::MoveTrashRest),
    ] {
        let hash = hash_of(name);
        let outcome = move_out(&f.conn, &f.app_root, &f.cache, ItemRef::Hash(&hash), &dest, mode)
            .unwrap();
        assert_eq!(outcome.exported, 1, "{name}");
    }

    assert!(!f.root.join("moved.jpg").exists());
    for (name, born) in [("copied.jpg", copied_born), ("moved.jpg", moved_born)] {
        let output = dest.join(name);
        assert_eq!(std::fs::metadata(&output).unwrap().modified().unwrap(), dated, "{name}");
        if cfg!(any(target_os = "macos", windows)) {
            assert_eq!(created(&output), born, "{name}");
        }
    }
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
fn dot_store_copy_overwrite_then_move_verifies_existing_output_and_only_cleans_sources() {
    let f = fixture("dot-store-copy-move");
    let source = f.root.join(".DS_Store");
    std::fs::write(&source, b"new fixture bytes").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let output = dest.join(".DS_Store");
    std::fs::write(&output, b"old fixture bytes").unwrap();
    let item = f.conn.query_row("SELECT content_hash, id FROM paths WHERE file_name = '.DS_Store'", [],
        |row| Ok(ItemIdentity { hash: row.get(0)?, path_id: Some(row.get(1)?) })).unwrap();
    let selection = std::slice::from_ref(&item);
    let review = move_batch(&f.conn, &f.app_root, &f.cache, selection, &dest,
        MoveOutMode::CopyKeepAll, &|| false, |_| {}).unwrap();
    assert!(review.requires_conflict_choice);
    assert_eq!(std::fs::read(&source).unwrap(), b"new fixture bytes");
    assert_eq!(std::fs::read(&output).unwrap(), b"old fixture bytes");
    let copied = move_batch_reviewed(&f.conn, &f.app_root, &f.cache, selection,
        &AcceptedFiles::capture(&f.conn, selection).unwrap(), &dest,
        MoveOutMode::CopyKeepAll, Some(DestinationConflictPolicy::Overwrite), review.plan_token.as_deref(),
        RenameStyle::SpaceNumber, &|| false, |_| {}).unwrap();
    assert_eq!(copied.exported, 1);
    assert_eq!(copied.trashed_destination_files, 1);
    assert_eq!(copied.post_action.deleted_files, 0);
    assert_eq!(std::fs::read(&source).unwrap(), b"new fixture bytes");
    assert_eq!(std::fs::read(&output).unwrap(), b"new fixture bytes");
    let moved = move_batch(&f.conn, &f.app_root, &f.cache, selection, &dest,
        MoveOutMode::MoveTrashRest, &|| false, |_| {}).unwrap();
    assert!(!moved.requires_conflict_choice);
    assert_eq!(moved.exported, 0);
    assert_eq!(moved.skipped_identical, 1);
    assert_eq!(moved.trashed_destination_files, 0);
    assert_eq!(moved.post_action.deleted_files, 1);
    assert!(!source.exists());
    assert_eq!(std::fs::read(&output).unwrap(), b"new fixture bytes");
    assert!(f.root.join(onecopy_lib::trash::TRASH_DIR_NAME).is_dir());
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
        &[item.clone()],
        &AcceptedFiles::capture(&f.conn, &[item]).unwrap(),
        &dest,
        MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Rename),
        review.plan_token.as_deref(),
        RenameStyle::ParenthesizedNumber,
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
        &[item.clone()],
        &AcceptedFiles::capture(&f.conn, &[item]).unwrap(),
        &dest,
        MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Overwrite),
        review.plan_token.as_deref(),
        RenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.exported, 2);
    assert_eq!(outcome.trashed_destination_files, 2);
    assert_eq!(outcome.post_action.deleted_files, 2);
    assert_eq!(std::fs::read(dest.join("x.jpg")).unwrap(), b"new-primary");
    assert_eq!(std::fs::read(dest.join("x.xmp")).unwrap(), b"new-sidecar");

    let day_dir = std::fs::read_dir(f._dir.path().join(onecopy_lib::trash::TRASH_DIR_NAME))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .expect("destination replacements are recoverable");
    let lines = std::fs::read_to_string(day_dir.join("manifest.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    // Overwrite records what it displaced with the hash the user reviewed.
    for line in &lines {
        assert_eq!(line["kind"], "overwrite-displaced");
        assert_eq!(line["item"], serde_json::Value::Null);
        let stored = std::fs::read(line["storedPath"].as_str().unwrap()).unwrap();
        assert_eq!(
            line["contentHash"].as_str().expect("the reviewed hash"),
            blake3::hash(&stored).to_hex().as_str()
        );
        let main = line["originalPath"].as_str().unwrap().ends_with("x.jpg");
        assert_eq!(line["role"], if main { "main" } else { "companion" });
    }
    let originals = lines
        .iter()
        .map(|line| line["originalPath"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(originals.contains(&dest.join("x.jpg").to_string_lossy().into_owned()));
    assert!(
        originals.contains(&dest.join("x.xmp").to_string_lossy().into_owned()),
        "trash manifest originals: {originals:?}"
    );
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
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |row| {
            row.get(0)
        })
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
        &[item.clone()],
        &AcceptedFiles::capture(&f.conn, &[item]).unwrap(),
        &dest,
        MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Overwrite),
        review.plan_token.as_deref(),
        RenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert!(outcome.plan_changed);
    assert!(outcome.items.is_empty());
    assert!(f.root.join("x.jpg").exists());
    assert_eq!(std::fs::read(dest.join("x.jpg")).unwrap(), b"old-version2");
    assert!(!f.root.join(onecopy_lib::trash::TRASH_DIR_NAME).exists());
    assert!(!f
        ._dir
        .path()
        .join(onecopy_lib::trash::TRASH_DIR_NAME)
        .exists());
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
        .query_row("SELECT COUNT(*) FROM active_issues", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        issues, 0,
        "the operation does not enforce the older indexed bytes"
    );
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
            "SELECT COUNT(*) FROM active_issues WHERE kind = 'delete-error'",
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
        &[item.clone()],
        &AcceptedFiles::capture(&f.conn, &[item]).unwrap(),
        &dest,
        MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Rename),
        review.plan_token.as_deref(),
        RenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.exported, 2);
    assert_eq!(outcome.post_action.deleted_files, 2);
    assert_eq!(
        std::fs::read(dest.join("x 2.jpg")).unwrap(),
        b"primary-bytes"
    );
    assert_eq!(std::fs::read(dest.join("x 2.arw")).unwrap(), b"raw-bytes");
    assert_eq!(
        std::fs::read(dest.join("x.arw")).unwrap(),
        b"a-different-raw"
    );
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
    assert!(
        !f.root.join("x.jpg").exists(),
        "the delivered primary is cleaned"
    );
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
            "SELECT COUNT(*) FROM active_issues WHERE kind = 'copy-error'",
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
        !f.root.join(onecopy_lib::trash::TRASH_DIR_NAME).exists(),
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
    let items = section_items(&f, "image");
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

    let items = section_items(&f, "other");
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

#[test]
fn deleting_one_empty_file_leaves_every_other_empty_file() {
    let f = fixture("empty-files");
    std::fs::create_dir_all(f.root.join("sub")).unwrap();
    for name in ["notes.txt", "other.txt", "sub/keep.txt", "blank.jpg"] {
        std::fs::write(f.root.join(name), b"").unwrap();
    }
    scan(&f);
    let (hash, path_id): (Option<String>, i64) = f
        .conn
        .query_row(
            "SELECT content_hash, id FROM paths WHERE file_name = 'notes.txt'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let item = ItemIdentity {
        path_id: hash.is_none().then_some(path_id),
        hash,
    };

    let outcome = delete_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item],
        DeleteMode::Permanent,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert_eq!(outcome.deleted_files, 1);
    assert!(!f.root.join("notes.txt").exists());
    for name in ["other.txt", "sub/keep.txt", "blank.jpg"] {
        assert!(f.root.join(name).exists(), "{name} must survive");
    }
}

fn rotten_copy_fixture(label: &str) -> (Fixture, String, std::path::PathBuf) {
    let f = fixture(label);
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
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    (f, hash, dest)
}

#[test]
fn move_skips_a_changed_copy_and_never_destroys_the_unchanged_one() {
    for mode in [MoveOutMode::MoveTrashRest, MoveOutMode::MoveDeleteRest] {
        let (f, hash, dest) = rotten_copy_fixture("rot-move");
        // The representative copy a changes after indexing: same length,
        // different bytes. Delivering it would cover b, the only copy left of
        // the reviewed content.
        std::fs::write(f.root.join("a").join("r.jpg"), b"rotten!-bytes").unwrap();

        let outcome =
            move_out(&f.conn, &f.app_root, &f.cache, ItemRef::Hash(&hash), &dest, mode).unwrap();

        assert_eq!(outcome.exported, 1);
        assert_eq!(std::fs::read(dest.join("r.jpg")).unwrap(), b"healthy-bytes");
        assert_eq!(
            std::fs::read(f.root.join("a").join("r.jpg")).unwrap(),
            b"rotten!-bytes",
            "the changed copy is not covered by the delivery and stays in place"
        );
        assert!(!f.root.join("b").join("r.jpg").exists());
        assert_eq!(outcome.post_action.deleted_files, 1);
        let issues: Vec<String> = f
            .conn
            .prepare("SELECT path FROM active_issues WHERE kind = 'copy-error'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].ends_with("r.jpg") && issues[0].contains("/a/"));
    }
}

#[test]
fn move_delivers_nothing_and_keeps_every_copy_when_every_copy_changed() {
    let (f, hash, dest) = rotten_copy_fixture("rot-all");
    for sub in ["a", "b"] {
        std::fs::write(f.root.join(sub).join("r.jpg"), b"rotten!-bytes").unwrap();
    }

    let outcome = move_out(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        &dest,
        MoveOutMode::MoveDeleteRest,
    )
    .unwrap();

    assert_eq!(outcome.exported, 0);
    assert_eq!(outcome.undelivered.len(), 1);
    assert!(!dest.join("r.jpg").exists());
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0, "no private output remains");
    for sub in ["a", "b"] {
        assert!(f.root.join(sub).join("r.jpg").exists());
    }
}

#[test]
fn copies_discovered_after_acceptance_never_join_a_confirmed_delete() {
    for mode in [DeleteMode::Trash, DeleteMode::Permanent] {
        let f = fixture("accepted-delete");
        // Two known copies give the item its real content identity.
        std::fs::create_dir_all(f.root.join("twin")).unwrap();
        std::fs::write(f.root.join("reviewed.jpg"), b"same-image-bytes").unwrap();
        std::fs::write(f.root.join("twin").join("reviewed.jpg"), b"same-image-bytes").unwrap();
        scan(&f);
        let hash: String = f
            .conn
            .query_row("SELECT content_hash FROM paths WHERE file_name = 'reviewed.jpg'", [], |r| r.get(0))
            .unwrap();
        let items = [ItemIdentity { hash: Some(hash), path_id: None }];
        let accepted = AcceptedFiles::capture(&f.conn, &items).unwrap();

        // While the accepted operation waits for background work, indexing
        // commits a byte-identical copy on another drive.
        std::fs::create_dir_all(f.root.join("backup")).unwrap();
        std::fs::write(f.root.join("backup").join("late.jpg"), b"same-image-bytes").unwrap();
        scan(&f);

        let outcome = delete_accepted_batch(
            &f.conn, &f.app_root, &f.cache, &items, &accepted, mode, &|| false, |_| {},
        )
        .unwrap();

        assert_eq!(outcome.deleted_files, 2);
        assert!(!f.root.join("reviewed.jpg").exists());
        assert!(f.root.join("backup").join("late.jpg").exists(), "{mode:?}");
    }
}

#[test]
fn copies_discovered_after_acceptance_never_join_a_confirmed_move() {
    let f = fixture("accepted-move");
    std::fs::create_dir_all(f.root.join("twin")).unwrap();
    std::fs::write(f.root.join("reviewed.jpg"), b"same-image-bytes").unwrap();
    std::fs::write(f.root.join("twin").join("reviewed.jpg"), b"same-image-bytes").unwrap();
    scan(&f);
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths WHERE file_name = 'reviewed.jpg'", [], |r| r.get(0))
        .unwrap();
    let items = [ItemIdentity { hash: Some(hash), path_id: None }];
    let accepted = AcceptedFiles::capture(&f.conn, &items).unwrap();
    std::fs::create_dir_all(f.root.join("backup")).unwrap();
    std::fs::write(f.root.join("backup").join("late.jpg"), b"same-image-bytes").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();

    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        &items,
        &accepted,
        &dest,
        MoveOutMode::MoveDeleteRest,
        None,
        None,
        RenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert_eq!(outcome.exported, 1);
    assert_eq!(std::fs::read(dest.join("reviewed.jpg")).unwrap(), b"same-image-bytes");
    assert_eq!(outcome.post_action.deleted_files, 2);
    assert!(!f.root.join("reviewed.jpg").exists());
    assert!(f.root.join("backup").join("late.jpg").exists());
}

fn write_config(f: &Fixture, sources: &[&std::path::Path], destinations: &[&std::path::Path]) {
    let list = |paths: &[&std::path::Path]| {
        paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    std::fs::write(
        f.app_root.join("config.json"),
        serde_json::to_vec(&serde_json::json!({
            "sourceDirs": list(sources),
            "destinationRoots": list(destinations),
        }))
        .unwrap(),
    )
    .unwrap();
}

fn item_named(f: &Fixture, file_name: &str) -> ItemIdentity {
    let hash: String = f
        .conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = ?1",
            [file_name],
            |row| row.get(0),
        )
        .unwrap();
    ItemIdentity {
        hash: Some(hash),
        path_id: None,
    }
}

fn private_leftovers(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".onecopy-") || name.ends_with(".tmp"))
        .collect()
}

#[test]
fn an_unavailable_source_or_destination_root_does_not_block_another_destination() {
    let f = fixture("offline-roots");
    std::fs::write(f.root.join("keep.jpg"), b"bytes").unwrap();
    scan(&f);
    let offline_source = f._dir.path().join("unplugged-source");
    let offline_destination = f._dir.path().join("unplugged-destination");
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    write_config(
        &f,
        &[&offline_source, &f.root],
        &[&offline_destination, f._dir.path()],
    );

    let outcome = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item_named(&f, "keep.jpg")],
        &dest,
        MoveOutMode::CopyKeepAll,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert_eq!(outcome.error, None);
    assert_eq!(outcome.exported, 1);
    assert_eq!(std::fs::read(dest.join("keep.jpg")).unwrap(), b"bytes");
}

#[test]
fn destination_admission_refuses_a_source_folder_and_an_unconfigured_folder() {
    let f = fixture("admission");
    std::fs::write(f.root.join("keep.jpg"), b"bytes").unwrap();
    scan(&f);
    let inside_source = f.root.join("sub");
    std::fs::create_dir_all(&inside_source).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let item = item_named(&f, "keep.jpg");

    for (dest, expected) in [
        (inside_source.as_path(), "inside the scanned directory"),
        (elsewhere.path(), "not a configured destination root"),
        (f._dir.path().join("absent").as_path(), "not a directory"),
    ] {
        let error = move_batch(
            &f.conn,
            &f.app_root,
            &f.cache,
            std::slice::from_ref(&item),
            dest,
            MoveOutMode::CopyKeepAll,
            &|| false,
            |_| {},
        )
        .unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    assert_eq!(std::fs::read_dir(&inside_source).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    assert!(f.root.join("keep.jpg").exists());
}

#[test]
fn a_copy_whose_owner_cannot_be_established_fails_only_itself() {
    // A copy under a source removed in Settings (or on a drive whose owner
    // cannot be resolved) is one unavailable file, not a planning failure.
    let f = fixture("unowned-copy");
    let removed = f._dir.path().join("removed-source");
    std::fs::create_dir_all(&removed).unwrap();
    std::fs::write(f.root.join("a.jpg"), b"same").unwrap();
    std::fs::write(removed.join("a.jpg"), b"same").unwrap();
    scanner::walk_root(&f.conn, &removed, &lists()).unwrap();
    scan(&f);
    let dest_root = f._dir.path().join("destinations");
    std::fs::create_dir_all(&dest_root).unwrap();
    write_config(&f, &[&f.root], &[&dest_root]);
    let item = item_named(&f, "a.jpg");

    let outcome = delete_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        std::slice::from_ref(&item),
        DeleteMode::Trash,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert_eq!(outcome.error, None);
    assert_eq!(outcome.deleted_files, 1);
    assert_eq!(outcome.failed_files, 1);
    assert!(!f.root.join("a.jpg").exists());
    assert!(removed.join("a.jpg").exists());
}

#[test]
fn a_missing_file_fails_as_itself_in_both_deletion_modes() {
    for mode in [DeleteMode::Trash, DeleteMode::Permanent] {
        let f = fixture("missing-delete");
        std::fs::write(f.root.join("gone.jpg"), b"bytes").unwrap();
        scan(&f);
        let item = item_named(&f, "gone.jpg");
        std::fs::remove_file(f.root.join("gone.jpg")).unwrap();

        let outcome = delete_batch(
            &f.conn,
            &f.app_root,
            &f.cache,
            &[item],
            mode,
            &|| false,
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome.deleted_files, 0, "{mode:?}");
        assert_eq!(outcome.failed_files, 1, "{mode:?}");
    }
}

#[test]
fn case_only_name_collisions_in_the_selection_are_reviewed_like_the_destination_compares() {
    let f = fixture("case-collision");
    for (dir, name, bytes) in [("a", "IMG.JPG", b"first".as_slice()), ("b", "img.jpg", b"second".as_slice())] {
        std::fs::create_dir_all(f.root.join(dir)).unwrap();
        std::fs::write(f.root.join(dir).join(name), bytes).unwrap();
    }
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let items = vec![item_named(&f, "IMG.JPG"), item_named(&f, "img.jpg")];
    let folds_case = FolderNames::for_directory(&dest).folds_case();

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

    if !folds_case {
        assert!(!review.requires_conflict_choice);
        assert_eq!(review.exported, 2);
        return;
    }
    assert!(review.requires_conflict_choice, "the collision is presented before any work");
    assert!(!review.overwrite_allowed);
    assert!(review.reviewed_conflicts.iter().all(|conflict| conflict.within_selection));
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0);

    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        &items,
        &AcceptedFiles::capture(&f.conn, &items).unwrap(),
        &dest,
        MoveOutMode::CopyKeepAll,
        Some(DestinationConflictPolicy::Rename),
        review.plan_token.as_deref(),
        RenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.exported, 2);
    assert_eq!(std::fs::read(dest.join("IMG.JPG")).unwrap(), b"first");
    assert_eq!(std::fs::read(dest.join("img 2.jpg")).unwrap(), b"second");
}

#[test]
#[cfg(target_os = "macos")]
fn nfc_nfd_name_collisions_are_reviewed_like_the_destination_compares() {
    // APFS and HFS+ normalize names on the way to disk, so an NFC-composed
    // name and its NFD-decomposed equivalent are the same destination entry,
    // even on a case-sensitive volume. This pair must be caught at planning,
    // not left to fail when publication discovers the destination already
    // holds the other spelling.
    let nfc_name = "caf\u{00e9}.jpg"; // "café", é as U+00E9
    let nfd_name = "cafe\u{0301}.jpg"; // "café", e + combining acute U+0301
    let f = fixture("nfc-nfd-collision");
    for (dir, name, bytes) in [
        ("a", nfc_name, b"first".as_slice()),
        ("b", nfd_name, b"second".as_slice()),
    ] {
        std::fs::create_dir_all(f.root.join(dir)).unwrap();
        std::fs::write(f.root.join(dir).join(name), bytes).unwrap();
    }
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let items = vec![item_named(&f, nfc_name), item_named(&f, nfd_name)];

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

    assert!(review.requires_conflict_choice, "the collision is presented before any work");
    assert!(!review.overwrite_allowed);
    assert!(review.reviewed_conflicts.iter().all(|conflict| conflict.within_selection));
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0);

    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        &items,
        &AcceptedFiles::capture(&f.conn, &items).unwrap(),
        &dest,
        MoveOutMode::CopyKeepAll,
        Some(DestinationConflictPolicy::Rename),
        review.plan_token.as_deref(),
        RenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.exported, 2);
}

#[test]
fn a_name_the_destination_refuses_fails_only_that_file_and_long_names_still_stage() {
    let f = fixture("long-names");
    // A 255-byte name fits the destination; its private name must too.
    let long = format!("{}.jpg", "x".repeat(251));
    std::fs::write(f.root.join(&long), b"long-name").unwrap();
    std::fs::write(f.root.join("short.jpg"), b"short-name").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let items = vec![item_named(&f, &long), item_named(&f, "short.jpg")];

    let copied = move_batch(
        &f.conn, &f.app_root, &f.cache, &items, &dest,
        MoveOutMode::CopyKeepAll, &|| false, |_| {},
    )
    .unwrap();
    assert_eq!(copied.error, None);
    assert_eq!(copied.exported, 2);

    // Renaming the long name past the filesystem limit fails only it. The
    // copy settled each item's identity, so name them again.
    let items = vec![item_named(&f, &long), item_named(&f, "short.jpg")];
    std::fs::write(dest.join(&long), b"occupied").unwrap();
    std::fs::remove_file(dest.join("short.jpg")).unwrap();
    let review = move_batch(
        &f.conn, &f.app_root, &f.cache, &items, &dest,
        MoveOutMode::CopyKeepAll, &|| false, |_| {},
    )
    .unwrap();
    assert!(review.requires_conflict_choice, "{review:?}");
    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        &items,
        &AcceptedFiles::capture(&f.conn, &items).unwrap(),
        &dest,
        MoveOutMode::CopyKeepAll,
        Some(DestinationConflictPolicy::Rename),
        review.plan_token.as_deref(),
        RenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.undelivered.len(), 1);
    assert_eq!(outcome.exported, 1);
    assert_eq!(std::fs::read(dest.join("short.jpg")).unwrap(), b"short-name");
    assert_eq!(std::fs::read(dest.join(&long)).unwrap(), b"occupied");
    assert!(private_leftovers(&dest).is_empty(), "{:?}", private_leftovers(&dest));
}

#[cfg(unix)]
#[test]
fn overwrite_displaces_nothing_until_the_complete_replacement_is_prepared() {
    use std::os::unix::fs::PermissionsExt;

    let f = fixture("overwrite-incomplete");
    std::fs::write(f.root.join("x.jpg"), b"new-primary").unwrap();
    std::fs::write(f.root.join("x.xmp"), b"new-sidecar").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("x.jpg"), b"old-primary").unwrap();
    std::fs::write(dest.join("x.xmp"), b"old-sidecar").unwrap();
    let item = item_named(&f, "x.jpg");
    let review = move_batch(
        &f.conn, &f.app_root, &f.cache, std::slice::from_ref(&item), &dest,
        MoveOutMode::MoveTrashRest, &|| false, |_| {},
    )
    .unwrap();
    assert!(review.overwrite_allowed);
    // The incoming sidecar cannot be read when the operation runs.
    std::fs::set_permissions(f.root.join("x.xmp"), std::fs::Permissions::from_mode(0o000)).unwrap();

    let outcome = move_batch_reviewed(
        &f.conn,
        &f.app_root,
        &f.cache,
        std::slice::from_ref(&item),
        &AcceptedFiles::capture(&f.conn, std::slice::from_ref(&item)).unwrap(),
        &dest,
        MoveOutMode::MoveTrashRest,
        Some(DestinationConflictPolicy::Overwrite),
        review.plan_token.as_deref(),
        RenameStyle::SpaceNumber,
        &|| false,
        |_| {},
    )
    .unwrap();
    std::fs::set_permissions(f.root.join("x.xmp"), std::fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(outcome.error, None);
    assert_eq!(outcome.trashed_destination_files, 0);
    assert_eq!(outcome.exported, 0);
    assert_eq!(std::fs::read(dest.join("x.jpg")).unwrap(), b"old-primary");
    assert_eq!(std::fs::read(dest.join("x.xmp")).unwrap(), b"old-sidecar");
    assert!(f.root.join("x.jpg").exists());
    assert!(f.root.join("x.xmp").exists());
    assert!(private_leftovers(&dest).is_empty(), "{:?}", private_leftovers(&dest));
}

#[test]
fn move_out_modes_have_one_wire_name() {
    use onecopy_lib::operations::MoveOutMode;
    for (mode, wire) in [
        (MoveOutMode::MoveTrashRest, "move-trash-rest"),
        (MoveOutMode::MoveDeleteRest, "move-delete-rest"),
        (MoveOutMode::CopyKeepAll, "copy"),
    ] {
        assert_eq!(mode.as_str(), wire);
        assert_eq!(serde_json::to_value(mode).unwrap(), wire);
        assert_eq!(serde_json::from_value::<MoveOutMode>(serde_json::json!(wire)).unwrap(), mode);
    }
}

#[test]
fn move_keeps_a_changed_copys_companion_beside_it() {
    for mode in [MoveOutMode::MoveTrashRest, MoveOutMode::MoveDeleteRest] {
        let (f, _, dest) = rotten_copy_fixture("rot-companion");
        for sub in ["a", "b"] {
            std::fs::write(
                f.root.join(sub).join("r.xmp"),
                format!("{sub}-sidecar").as_bytes(),
            )
            .unwrap();
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
        // The representative copy a changes after indexing; it is skipped and
        // stays in place, so its own sidecar must stay beside it.
        std::fs::write(f.root.join("a").join("r.jpg"), b"rotten!-bytes").unwrap();

        let outcome =
            move_out(&f.conn, &f.app_root, &f.cache, ItemRef::Hash(&hash), &dest, mode).unwrap();

        assert_eq!(std::fs::read(dest.join("r.jpg")).unwrap(), b"healthy-bytes");
        assert_eq!(
            std::fs::read(dest.join("r.xmp")).unwrap(),
            b"b-sidecar",
            "the delivered main copy's own sidecar is the companion output"
        );
        assert_eq!(
            std::fs::read(f.root.join("a").join("r.xmp")).unwrap(),
            b"a-sidecar",
            "the changed copy's sidecar stays beside it"
        );
        assert!(f.root.join("a").join("r.jpg").exists());
        assert!(!f.root.join("b").join("r.jpg").exists());
        assert!(!f.root.join("b").join("r.xmp").exists());
        assert_eq!(outcome.exported, 2);
    }
}

#[test]
fn deleting_the_last_live_copy_forgets_missing_siblings_that_carry_evidence() {
    // A missing sibling keeps the date evidence it was indexed with. Foreign
    // keys are enforced on every connection, so forgetting that sibling with
    // the identity must take its evidence first, or the whole delete fails
    // after the file already left the disk.
    let f = fixture("missing-sibling-evidence");
    for sub in ["a", "b"] {
        std::fs::create_dir_all(f.root.join(sub)).unwrap();
        std::fs::write(f.root.join(sub).join("IMG_20240102_030405.jpg"), b"same-bytes").unwrap();
    }
    scan(&f);
    scanner::extract_pending(&f.conn).unwrap();
    let evidence: i64 = f
        .conn
        .query_row("SELECT COUNT(*) FROM evidence", [], |r| r.get(0))
        .unwrap();
    assert!(evidence >= 2, "both copies carry filename evidence");
    let hash: String = f
        .conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();

    std::fs::remove_file(f.root.join("b").join("IMG_20240102_030405.jpg")).unwrap();
    scanner::walk_root(&f.conn, &f.root, &lists()).unwrap();

    delete_item(
        &f.conn,
        &f.app_root,
        &f.cache,
        ItemRef::Hash(&hash),
        DeleteMode::Trash,
    )
    .unwrap();
    let remaining: (i64, i64, i64) = f
        .conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM paths), (SELECT COUNT(*) FROM contents), \
             (SELECT COUNT(*) FROM evidence)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(remaining, (0, 0, 0));
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
fn an_unsettled_operation_sweeps_nothing_from_the_destination_folder() {
    // Ordinary Copy/Move staging lands flat in the destination folder, which
    // the library's source walk never visits (it is outside every configured
    // source). `move_batch` is therefore the one place a crash's leftover
    // staging there ever gets cleaned up — but only once this process's own
    // application-home identity is proven (a settled data root with a
    // readable installation id; `file_identity::is_abandoned_leftover`).
    // This test binary never settles `paths::data_root` (like
    // `scanner_tests.rs` and `file_identity_tests.rs`, deliberately: it is
    // one process-global OnceLock shared by every test in the binary, so no
    // single test can settle it without racing every other one), so this
    // process has no proven fingerprint and sweeps nothing here, not even a
    // leftover that carries its own (unsettled-placeholder) home tag and a
    // dead pid. What this proves at the operation level is that `move_batch`
    // never removes a live process's file or a different application home's
    // file sharing the same destination folder, and that its own dead-pid-
    // looking leftover survives too while unproven. The complementary,
    // settled-home half — that such a leftover actually gets removed once
    // proven — is exercised directly, with an injected fingerprint, by
    // `sweep_removes_only_this_installations_dead_leftover` in
    // `tests/unit/file_identity.rs`.
    let f = fixture("destination-sweep");
    std::fs::write(f.root.join("keep.jpg"), b"bytes").unwrap();
    scan(&f);
    let dest = f._dir.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    write_config(&f, &[&f.root], &[dest.as_path()]);

    let base = onecopy_lib::file_identity::private_stage_file_name().unwrap();
    let dead = dest.join(with_pid(&base, exited_pid()));
    let live = dest.join(with_pid(&base, std::process::id()));
    let foreign = dest.join(with_foreign_home(&with_pid(&base, exited_pid())));
    for path in [&dead, &live, &foreign] {
        std::fs::write(path, b"leftover").unwrap();
    }

    let outcome = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item_named(&f, "keep.jpg")],
        &dest,
        MoveOutMode::CopyKeepAll,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert_eq!(outcome.error, None);
    assert_eq!(outcome.exported, 1);
    assert!(
        dead.exists(),
        "an unsettled process has no proven identity, so even its own dead-pid-looking leftover survives"
    );
    assert!(live.exists(), "a live process's own file is never removed");
    assert!(foreign.exists(), "a different application home's file is never removed");
    let remaining = private_leftovers(&dest);
    for path in [&dead, &live, &foreign] {
        assert!(
            remaining.iter().any(|name| name == path.file_name().unwrap().to_str().unwrap()),
            "expected {path:?} to remain: {remaining:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A volume that stops answering (volume_io's fake stalling volume)

use onecopy_lib::volume_io::{FakeStallingVolume, Op};

const STALL_BOUND: std::time::Duration = std::time::Duration::from_millis(300);
const SETTLE: std::time::Duration = std::time::Duration::from_secs(10);

fn open_issue_kinds(f: &Fixture, path: &std::path::Path) -> Vec<String> {
    let mut statement = f
        .conn
        .prepare("SELECT kind FROM active_issues WHERE path = ?1 ORDER BY kind")
        .unwrap();
    statement
        .query_map([path.to_string_lossy()], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn row_state(f: &Fixture, file_name: &str) -> Option<i64> {
    use rusqlite::OptionalExtension;
    f.conn
        .query_row(
            "SELECT missing FROM paths WHERE file_name = ?1",
            [file_name],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

/// Moves with the destination's publication given up on; `land` decides
/// whether the stalled rename then completes or fails.
fn move_with_publication_given_up(label: &str, land: bool) {
    let f = fixture(label);
    std::fs::write(f.root.join("photo.jpg"), b"image-bytes").unwrap();
    scan(&f);
    let dest = f._dir.path().join("destination");
    std::fs::create_dir(&dest).unwrap();
    let volume = FakeStallingVolume::mount(&dest, STALL_BOUND);
    // The private claim is a rename too; the publication is the next one.
    volume.stall_after(&[Op::Rename], None, 1);

    let outcome = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item_named(&f, "photo.jpg")],
        &dest,
        MoveOutMode::MoveTrashRest,
        &|| false,
        |_| {},
    )
    .unwrap();

    let target = dest.join("photo.jpg");
    assert_eq!(outcome.unknown, vec![target.to_string_lossy().into_owned()]);
    assert_eq!(outcome.exported, 0);
    assert!(outcome.error.is_some(), "nothing further goes to a stalled destination");
    // A Move never handles a source after an unknown publication.
    assert_eq!(outcome.post_action.deleted_files, 0);
    assert_eq!(std::fs::read(f.root.join("photo.jpg")).unwrap(), b"image-bytes");
    assert_eq!(row_state(&f, "photo.jpg"), Some(0));
    assert_eq!(open_issue_kinds(&f, &target), vec![COPY_OUTCOME_UNKNOWN]);

    if land {
        volume.release();
    } else {
        volume.fail();
    }
    assert!(volume.wait_until_settled(SETTLE));
    if land {
        assert_eq!(std::fs::read(&target).unwrap(), b"image-bytes");
    } else {
        assert!(!target.exists());
    }
    assert!(private_leftovers(&dest).is_empty(), "{:?}", private_leftovers(&dest));
    // Either way the source is still there: a duplicate, never a loss.
    assert!(f.root.join("photo.jpg").exists());
}

#[test]
fn a_publication_given_up_on_that_lands_later_leaves_a_complete_copy_and_the_source() {
    move_with_publication_given_up("publish-lands", true);
}

#[test]
fn a_publication_given_up_on_that_fails_later_leaves_nothing_public_or_private() {
    move_with_publication_given_up("publish-fails", false);
}

fn delete_with_move_given_up(label: &str, mode: DeleteMode, land: bool) {
    let f = fixture(label);
    std::fs::write(f.root.join("photo.jpg"), b"image-bytes").unwrap();
    scan(&f);
    let item = item_named(&f, "photo.jpg");
    let volume = FakeStallingVolume::mount(&f.root, STALL_BOUND);
    volume.stall(
        &[match mode {
            DeleteMode::Trash => Op::Rename,
            DeleteMode::Permanent => Op::Remove,
        }],
        None,
    );

    let hash = item.hash.clone().unwrap();
    let outcome =
        delete_item(&f.conn, &f.app_root, &f.cache, ItemRef::Hash(&hash), mode).unwrap();

    assert_eq!(outcome.deleted_files, 0);
    assert_eq!(outcome.unknown_files, 1);
    assert_eq!(outcome.removed_rows, 0, "the row waits for the next source check");
    assert_eq!(row_state(&f, "photo.jpg"), Some(0));
    assert_eq!(
        open_issue_kinds(&f, &f.root.join("photo.jpg")),
        vec![DELETE_OUTCOME_UNKNOWN]
    );
    if mode == DeleteMode::Trash {
        // Provenance was committed before the move was attempted.
        let trash = f.root.join(onecopy_lib::trash::TRASH_DIR_NAME);
        let day = std::fs::read_dir(&trash).unwrap().next().unwrap().unwrap().path();
        let manifest = std::fs::read_to_string(day.join("manifest.jsonl")).unwrap();
        assert!(manifest.contains("photo.jpg"));
    }

    if land {
        volume.release();
    } else {
        volume.fail();
    }
    assert!(volume.wait_until_settled(SETTLE));
    assert_eq!(f.root.join("photo.jpg").exists(), !land);
    // The next source check settles the row either way.
    scanner::walk_root(&f.conn, &f.root, &lists()).unwrap();
    assert_eq!(row_state(&f, "photo.jpg"), Some(i64::from(land)));
}

#[test]
fn a_trash_move_given_up_on_keeps_the_row_until_the_next_check_finds_it_moved() {
    delete_with_move_given_up("trash-lands", DeleteMode::Trash, true);
}

#[test]
fn a_trash_move_given_up_on_keeps_the_row_until_the_next_check_finds_it_in_place() {
    delete_with_move_given_up("trash-fails", DeleteMode::Trash, false);
}

#[test]
fn a_permanent_delete_given_up_on_keeps_the_row_until_the_next_check_settles_it() {
    delete_with_move_given_up("remove-lands", DeleteMode::Permanent, true);
    delete_with_move_given_up("remove-fails", DeleteMode::Permanent, false);
}

#[test]
fn a_stalled_source_volume_does_not_stop_deletes_on_a_healthy_one() {
    let f = fixture("stalled-and-healthy");
    let stalled_root = f._dir.path().join("stalled");
    std::fs::create_dir(&stalled_root).unwrap();
    std::fs::write(stalled_root.join("away.jpg"), b"away-bytes").unwrap();
    std::fs::write(f.root.join("here.jpg"), b"here-bytes").unwrap();
    write_config(&f, &[&stalled_root, &f.root], &[f._dir.path()]);
    scan(&f);
    scanner::walk_root(&f.conn, &stalled_root, &lists()).unwrap();
    scanner::hash_pending(&f.conn, &f.cache).unwrap();
    let items = [item_named(&f, "away.jpg"), item_named(&f, "here.jpg")];
    let volume = FakeStallingVolume::mount(&stalled_root, STALL_BOUND);
    volume.stall(&[], None);

    let started = std::time::Instant::now();
    let outcome = delete_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &items,
        DeleteMode::Trash,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(outcome.deleted_files, 1);
    assert_eq!(outcome.failed_files, 1);
    assert_eq!(outcome.unknown_files, 0, "a check that never ran has a known outcome");
    assert!(!f.root.join("here.jpg").exists());
    assert_eq!(row_state(&f, "away.jpg"), Some(0));
    volume.release();
    assert!(volume.wait_until_settled(SETTLE));
    assert!(stalled_root.join("away.jpg").exists());
}

#[test]
fn a_move_whose_source_cleanup_is_given_up_on_reports_it_as_outcome_unknown() {
    let f = fixture("cleanup-unknown");
    std::fs::write(f.root.join("photo.jpg"), b"image-bytes").unwrap();
    scan(&f);
    let dest = f._dir.path().join("destination");
    std::fs::create_dir(&dest).unwrap();
    let volume = FakeStallingVolume::mount(&f.root, STALL_BOUND);
    // The source's move into Deleted files is the only rename on the source.
    volume.stall(&[Op::Rename], None);

    let outcome = move_batch(
        &f.conn,
        &f.app_root,
        &f.cache,
        &[item_named(&f, "photo.jpg")],
        &dest,
        MoveOutMode::MoveTrashRest,
        &|| false,
        |_| {},
    )
    .unwrap();

    assert_eq!(outcome.exported, 1);
    assert_eq!(outcome.post_action.deleted_files, 0);
    assert_eq!(outcome.post_action.unknown_files, 1);
    assert_eq!(
        open_issue_kinds(&f, &f.root.join("photo.jpg")),
        vec![DELETE_OUTCOME_UNKNOWN]
    );
    volume.release();
    assert!(volume.wait_until_settled(SETTLE));
}

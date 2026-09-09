use onecopy_lib::{
    index_store, queries, scanner,
    visibility::{self, Policy},
    visibility_index,
};
use rusqlite::params;
use serde_json::json;

fn lists() -> scanner::ScanLists {
    scanner::ScanLists {
        images: vec!["jpg".into()],
        videos: vec![],
        audio: vec![],
        companions: vec![],
    }
}

#[test]
fn policy_defaults_empty_list_exact_names_and_native_platform_facts() {
    let defaults = Policy::from_config(&json!({})).unwrap();
    assert!(!defaults.visible("tHuMbS.DB", false, 0));
    assert!(defaults.visible("Thumbs.db", true, 0));
    assert!(defaults.visible("myThumbs.db", false, 0));
    assert!(!defaults.visible("photo.jpg", false, visibility::windows_flags(2)));
    assert!(!defaults.visible("folder", true, visibility::windows_flags(4)));
    assert_eq!(visibility::macos_flags(0x8000), visibility::HIDDEN);
    assert_eq!(visibility::macos_flags(0x2), 0);
    let empty = Policy::from_config(&json!({"ignoredFileNames": []})).unwrap();
    assert!(empty.visible("Thumbs.db", false, 0));
    let literal = Policy::from_config(&json!({"ignoredFileNames": ["*.jpg"]})).unwrap();
    assert!(literal.visible("photo.jpg", false, 0));
    for bad in [
        json!({"ignoredFileNames": null}),
        json!({"ignoredFileNames": [1]}),
        json!({"ignoredFileNames": ["dir/file"]}),
        json!({"hideDotNames": "false"}),
    ] {
        assert!(Policy::from_config(&bad).is_err());
    }
}

#[test]
fn hidden_only_audio_is_not_background_transcription_work() {
    let temp = tempfile::tempdir().unwrap();
    let conn = index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('audio', 3, 'audio');
        INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, visibility_flags)
          VALUES ('/root/.clip.wav', '/root', '.clip.wav', 'audio', 'audio', 1);",
    )
    .unwrap();
    assert!(
        onecopy_lib::derived_state::transcript_candidates(&conn, "audio", None, 10)
            .unwrap()
            .is_empty()
    );
    visibility_index::apply_policy(
        &conn,
        &Policy::from_config(&json!({"hideDotNames": false})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        onecopy_lib::derived_state::transcript_candidates(&conn, "audio", None, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn explicitly_configured_nested_dot_root_is_visible_in_either_scan_order() {
    for nested_first in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("source");
        let inner = outer.join(".chosen");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("photo.jpg"), b"image").unwrap();
        let roots = if nested_first {
            vec![&inner, &outer]
        } else {
            vec![&outer, &inner]
        };
        let config = json!({"sourceDirs": roots});
        let settings = scanner::settings_from_config(Some(&config), temp.path(), 0);
        let conn = index_store::open(&temp.path().join("index.sqlite3")).unwrap();
        scanner::run_source_check(&conn, &settings, &|_| {}).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT review_visible FROM paths WHERE missing = 0",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
}

#[test]
fn visible_name_does_not_replace_hidden_copy_date_or_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(".explicit-root");
    std::fs::create_dir(&root).unwrap();
    for name in ["photo.jpg", ".photo.jpg"] {
        std::fs::write(root.join(name), b"same-image").unwrap();
    }
    let conn = index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    scanner::walk_root(&conn, &root, &lists()).unwrap();
    scanner::hash_pending(
        &conn,
        &onecopy_lib::preview::CachePaths::new(temp.path().join("cache")),
    )
    .unwrap();
    conn.execute("UPDATE paths SET resolved_source = 'filename', resolved_utc_ms = CASE WHEN file_name = '.photo.jpg' THEN 1000 ELSE 2000 END, date_only = (file_name = '.photo.jpg')", []).unwrap();
    let hash: String = conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let detail = queries::item_detail(&conn, Some(&hash), None).unwrap();
    assert_eq!(detail.file_name, "photo.jpg");
    assert_eq!(detail.resolved_utc_ms, Some(1000));
    assert!(detail.date_only);
    assert_eq!(detail.copy_paths.len(), 2);
    conn.execute(
        "UPDATE paths SET hash_attempt_failed = 1, metadata_attempt_failed = 1",
        [],
    )
    .unwrap();
    visibility_index::apply_policy(
        &conn,
        &Policy::from_config(&json!({"hideDotNames": false})).unwrap(),
    )
    .unwrap();
    let detail = queries::item_detail(&conn, Some(&hash), None).unwrap();
    assert_eq!(detail.file_name, ".photo.jpg");
    assert_eq!(detail.resolved_utc_ms, Some(1000));
    assert_eq!(
        conn.query_row(
            "SELECT SUM(hash_attempt_failed + metadata_attempt_failed) FROM paths",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        4
    );
    visibility_index::apply_policy(
        &conn,
        &Policy::from_config(&json!({"ignoredFileNames": ["photo.jpg"]})).unwrap(),
    )
    .unwrap();
    assert!(
        queries::live_content_hashes(&conn, std::slice::from_ref(&hash))
            .unwrap()
            .is_empty()
    );
    assert!(queries::item_detail(&conn, Some(&hash), None).is_err());
    assert_eq!(
        conn.query_row("SELECT live_copy_count FROM logical_contents", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        2
    );
    assert!(root.join("photo.jpg").exists());
    assert!(root.join(".photo.jpg").exists());
    visibility_index::apply_policy(&conn, &Policy::from_config(&json!({})).unwrap()).unwrap();
    assert_eq!(
        queries::live_content_hashes(&conn, &[hash]).unwrap().len(),
        1
    );
}

#[test]
fn directory_visibility_is_inherited_without_pruning_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir_all(root.join(".hidden/deep")).unwrap();
    std::fs::write(root.join(".hidden/deep/note.txt"), b"note").unwrap();
    let conn = index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    scanner::walk_root(&conn, &root, &lists()).unwrap();
    assert_eq!(
        conn.query_row("SELECT review_visible FROM paths", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    visibility_index::apply_policy(
        &conn,
        &Policy::from_config(&json!({"hideDotNames": false})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        conn.query_row("SELECT review_visible FROM paths", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let path = root.join(".hidden/deep/note.txt");
    let id: i64 = conn
        .query_row("SELECT id FROM paths", [], |row| row.get(0))
        .unwrap();
    conn.execute(
        "UPDATE paths SET visibility_checked = 0, visibility_flags = 0 WHERE id = ?1",
        [id],
    )
    .unwrap();
    visibility_index::complete_missing_facts(&conn, &[root.to_string_lossy().into_owned()])
        .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT visibility_flags FROM paths WHERE abs_path = ?1",
            params![path.to_string_lossy()],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        visibility::DOT
    );
}

#[cfg(target_os = "macos")]
#[test]
fn native_folder_attribute_change_republishes_unchanged_descendants() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let folder = root.join("folder");
    std::fs::create_dir_all(folder.join("deep")).unwrap();
    std::fs::write(folder.join("deep/photo.jpg"), b"image").unwrap();
    let conn = index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    scanner::walk_root(&conn, &root, &lists()).unwrap();
    let roots = [root.to_string_lossy().into_owned()];
    for (attribute, expected) in [("hidden", 0), ("nohidden", 1)] {
        assert!(std::process::Command::new("/usr/bin/chflags")
            .args([attribute])
            .arg(&folder)
            .status()
            .unwrap()
            .success());
        assert!(onecopy_lib::watcher::restat_dir(&conn, &folder, &lists(), &roots).unwrap() > 0);
        assert_eq!(
            conn.query_row("SELECT review_visible FROM paths", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            expected
        );
    }
}

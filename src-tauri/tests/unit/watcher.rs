use super::generation_is_live;

#[test]
fn replacement_and_shutdown_each_retire_an_owned_generation() {
    assert!(generation_is_live(4, 4, false));
    assert!(!generation_is_live(5, 4, false));
    assert!(!generation_is_live(4, 4, true));
}

// (R4.3 finding 5) A directory the watcher could not re-read leaves its part
// of the library stale, so the pass carries a failure to report instead of
// letting the loop clear the watcher-failed condition.
#[test]
fn a_directory_that_fails_to_restat_keeps_the_watcher_failure_visible() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let root = dir.path().join("watched");
    let outside = dir.path().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("new.jpg"), b"fresh").unwrap();
    let stored_root = crate::winpath::for_fs(&root).to_string_lossy().into_owned();
    let settings = crate::scanner::settings_from_config(
        Some(&serde_json::json!({ "sourceDirs": [stored_root] })),
        dir.path(),
        0,
    );

    let pass = super::restat_batch(&conn, &[outside.clone(), root.clone()], &settings, &|| Ok(())).unwrap();

    assert_eq!(pass.changed, 1, "the other directory is still updated");
    let failure = pass.failure().expect("the failed directory is reported");
    assert!(failure.contains(&*outside.to_string_lossy()), "{failure}");

    let clean = super::restat_batch(&conn, &[root], &settings, &|| Ok(())).unwrap();
    assert!(clean.failure().is_none(), "a clean pass clears the condition");
}

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
    // A data root distinct from `root`/`outside` (R6-02's exclusion would
    // otherwise treat these watched directories as the app's own storage,
    // since they would sit inside it).
    let app_data = dir.path().join("app-data");
    let settings = crate::scanner::settings_from_config(
        Some(&serde_json::json!({ "sourceDirs": [stored_root] })),
        &app_data,
        0,
    );

    let pass = super::restat_batch(&conn, &[outside.clone(), root.clone()], &settings, &|| Ok(())).unwrap();

    assert_eq!(pass.changed, 1, "the other directory is still updated");
    let failure = pass.failure().expect("the failed directory is reported");
    assert!(failure.contains(&*outside.to_string_lossy()), "{failure}");

    let clean = super::restat_batch(&conn, &[root], &settings, &|| Ok(())).unwrap();
    assert!(clean.failure().is_none(), "a clean pass clears the condition");
}

#[test]
fn recovery_scope_uses_path_boundaries_and_includes_overlapping_configured_roots() {
    use std::path::PathBuf;
    let roots = vec!["/photos".to_string(), "/photos/sub".to_string(), "/photos-other".to_string()];
    assert_eq!(super::affected_roots(&roots, &[PathBuf::from("/photos/sub/day")]), roots[..2]);
    assert_eq!(super::affected_roots(&roots, &[PathBuf::from("/photos-extra")]), roots);
}

#[test]
fn watcher_section_scope_covers_descendants_but_not_sibling_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("photos");
    let sibling = temp.path().join("photos-other");
    std::fs::create_dir_all(root.join("child")).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(root.join("child/a.txt"), b"a").unwrap();
    std::fs::write(sibling.join("b.txt"), b"b").unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let roots: Vec<String> = [&root, &sibling].iter().map(|path| crate::winpath::for_fs(path).to_string_lossy().into_owned()).collect();
    let settings = crate::scanner::settings_from_config(Some(&serde_json::json!({ "sourceDirs": roots })), &temp.path().join("app"), 0);
    crate::scanner::run_source_check(&conn, &settings, &|_| {}).unwrap();
    conn.execute("UPDATE paths SET resolved_utc_ms = CASE WHEN file_name = 'a.txt' THEN 1767225600000 ELSE 1769904000000 END", []).unwrap();
    let stored = vec![crate::scanner::settled_root(&conn, &root).unwrap().to_string_lossy().into_owned()];
    let sections = crate::queries::sections_under_directories(&conn, &stored, chrono_tz::UTC).unwrap();
    assert_eq!(sections.len(), 1);
    assert!(sections.contains(&crate::queries::SectionLocation { kind: "other".to_string(), month: "2026-01".to_string() }));
    // Capturing before and after a changed date retains both affected months.
    conn.execute("UPDATE paths SET resolved_utc_ms = 1772323200000 WHERE file_name = 'a.txt'", []).unwrap();
    let mut both = sections;
    both.extend(crate::queries::sections_under_directories(&conn, &stored, chrono_tz::UTC).unwrap());
    assert_eq!(both.len(), 2);
    assert!(both.contains(&crate::queries::SectionLocation { kind: "other".to_string(), month: "2026-03".to_string() }));
}

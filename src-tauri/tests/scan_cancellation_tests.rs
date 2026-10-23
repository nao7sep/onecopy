use onecopy_lib::{index_store, scanner};
use std::sync::atomic::Ordering;

#[test]
#[serial_test::serial(scan_cancel)]
fn cancelled_pairing_preserves_the_previous_complete_projection() {
    let _reset = super::ResetScanCancellation;
    scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let corpus = root.path().join("corpus");
    std::fs::create_dir(&corpus).unwrap();
    std::fs::write(corpus.join("IMG.JPG"), b"jpeg").unwrap();
    std::fs::write(corpus.join("IMG.ARW"), b"raw").unwrap();
    let settings =
        scanner::settings_from_config(None, root.path(), chrono::Utc::now().timestamp_millis());
    scanner::walk_root(&conn, &corpus, &settings.lists).unwrap();
    scanner::pair_companions(&conn, true).unwrap();

    scanner::SCAN_CANCEL.store(true, Ordering::SeqCst);
    assert_eq!(
        scanner::pair_companions(&conn, false).unwrap_err(),
        scanner::CANCELLED
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM paths WHERE companion_of IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "cancellation happens before the atomic projection changes"
    );
}

#[test]
#[serial_test::serial(scan_cancel)]
fn cancelled_hash_discovery_releases_its_cohort_and_can_restart() {
    let _reset = super::ResetScanCancellation;
    scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let corpus = root.path().join("corpus");
    std::fs::create_dir(&corpus).unwrap();
    std::fs::write(corpus.join("a.bin"), b"same bytes").unwrap();
    std::fs::write(corpus.join("b.bin"), b"same bytes").unwrap();
    let settings = scanner::settings_from_config(None, root.path(), chrono::Utc::now().timestamp_millis());
    scanner::walk_root(&conn, &corpus, &settings.lists).unwrap();
    let cache = onecopy_lib::preview::CachePaths::new(root.path().join("cache"));
    scanner::SCAN_CANCEL.store(true, Ordering::SeqCst);
    assert_eq!(scanner::hash_pending(&conn, &cache).unwrap_err(), scanner::CANCELLED);
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_temp_master WHERE name = 'onecopy_hash_cohort'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
    assert_eq!(scanner::hash_pending(&conn, &cache).unwrap().full_hashed, 2);
}

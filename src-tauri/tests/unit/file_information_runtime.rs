use super::*;

#[test]
fn a_completed_run_leaves_no_queued_work_behind() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let photo = dir.path().join("photo.jpg");
    std::fs::write(&photo, b"data").unwrap();
    let lists = crate::scanner::ScanLists {
        images: vec!["jpg".into()],
        videos: vec![],
        audio: vec![],
        companions: vec![],
    };
    crate::scanner::upsert_file(&conn, &photo, &lists, 0).unwrap();
    assert!(crate::scanner::pending_index_work_exists(&conn).unwrap());

    let summary = complete_pending(&conn, |conn| {
        // The run settles every piece of debt it found.
        conn.execute("DELETE FROM paths", []).map_err(|error| error.to_string())?;
        Ok(crate::scanner::ScanSummary::default())
    })
    .unwrap();

    assert!(summary.is_some());
    assert!(!crate::scanner::pending_index_work_exists(&conn).unwrap());
    assert!(!snapshot().queued);
    assert!(complete_pending(&conn, |_| unreachable!()).unwrap().is_none());
}

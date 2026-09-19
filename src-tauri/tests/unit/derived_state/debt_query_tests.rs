use super::*;

#[test]
fn debt_snapshot_scans_one_logical_projection_and_never_probes_paths() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-debt-plan-")
        .tempdir()
        .unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let mut statement = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {}", work_debt_sql(true)))
        .unwrap();
    let details: Vec<String> = statement
        .query_map([], |row| row.get(3))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert_eq!(
        details
            .iter()
            .filter(|line| line.starts_with("SCAN "))
            .count(),
        1,
        "debt projection must aggregate one source scan: {details:?}"
    );
    assert!(
        details.iter().any(|line| line.contains("logical_contents")),
        "debt projection lost the maintained live-item truth: {details:?}"
    );
    assert!(
        details.iter().all(|line| !line.contains("paths")),
        "debt projection regressed to physical-path probes: {details:?}"
    );
}

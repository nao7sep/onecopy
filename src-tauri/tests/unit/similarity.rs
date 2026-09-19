use std::cell::Cell;

use super::*;

#[test]
fn dated_candidate_load_seeks_one_logical_month_without_regrouping_paths() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-similarity-plan-")
        .tempdir()
        .unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let mut statement = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {DATED_CANDIDATES_SQL}"))
        .unwrap();
    let details: Vec<String> = statement
        .query_map([1_704_067_200_000i64, 1_706_745_600_000i64], |row| {
            row.get(3)
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert!(
        details.iter().any(|line| line.contains("logical_contents")),
        "similarity lost the maintained logical source: {details:?}"
    );
    assert!(
        details.iter().any(|line| line.contains("resolved_utc_ms")),
        "similarity no longer seeks the requested UTC month: {details:?}"
    );
    assert!(
        details.iter().all(|line| !line.contains("paths")),
        "similarity regressed to physical-path grouping: {details:?}"
    );
    assert!(
        details.iter().all(|line| !line.contains("TEMP B-TREE")),
        "similarity candidate loading reintroduced sorting/grouping: {details:?}"
    );
}

#[test]
fn sparse_candidate_construction_remains_cancellable() {
    let phashes: Vec<i64> = (0..10_000).map(|value| value * 0x1_0001).collect();
    let times = vec![None; phashes.len()];
    let polls = Cell::new(0usize);
    let stopped = || {
        polls.set(polls.get() + 1);
        polls.get() >= 3
    };

    let error = cluster_by_appearance_cancellable(&phashes, &times, 4, 10, 90, 2, &stopped)
        .unwrap_err();
    assert_eq!(error, crate::scanner::CANCELLED);
    assert!(polls.get() >= 3);
}

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

    let error = cluster_by_appearance(&phashes, &times, 4, 10, 90, 2, &stopped).unwrap_err();
    assert_eq!(error, crate::scanner::CANCELLED);
    assert!(polls.get() >= 3);
}

#[test]
fn burst_leader_cannot_admit_endpoints_outside_their_pair_diameter() {
    let hashes = [0, 1023, 1_047_552];
    let times = [Some(90_000), Some(0), Some(180_000)];
    let groups = cluster_by_appearance(&hashes, &times, 3, 10, 90, 2, &|| false).unwrap();
    assert!(groups
        .iter()
        .all(|group| !(group.contains(&1) && group.contains(&2))));
    assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 3);
}

#[test]
fn dense_equal_signatures_need_linear_neighbor_operations_and_keep_every_member() {
    let count = 20_000;
    let hashes = vec![42; count];
    let times: Vec<_> = (0..count).map(|index| Some(index as i64)).collect();
    let mut uf = UnionFind::new(count);
    let mut comparisons = 0;
    union_strict_neighbors(&hashes, 3, &mut uf, &mut comparisons, &|| false).unwrap();
    union_burst_neighbors(
        &hashes,
        &times,
        3,
        10,
        90_000,
        &mut uf,
        &mut comparisons,
        &|| false,
    )
    .unwrap();
    assert_eq!(
        comparisons,
        count - 1,
        "duplicates bypass both candidate scans"
    );
    let members: Vec<_> = (0..count).collect();
    assert_eq!(
        summarize_members(&members, &hashes, &times, &|| false)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        cluster_by_appearance(&hashes, &times, 3, 10, 90, 2, &|| false).unwrap(),
        vec![members],
    );
}

#[test]
fn all_bit_threshold_unions_linearly() {
    let hashes: Vec<_> = (0..10_000).collect();
    let mut uf = UnionFind::new(hashes.len());
    let mut comparisons = 0;
    union_strict_neighbors(&hashes, 64, &mut uf, &mut comparisons, &|| false).unwrap();
    assert_eq!(comparisons, hashes.len() - 1);
    let root = uf.find(0);
    assert!((0..hashes.len()).all(|index| uf.find(index) == root));
}

#[test]
fn signature_date_summaries_match_independent_pair_enumeration() {
    let date_sets = [
        vec![None],
        vec![Some(0)],
        vec![Some(0), Some(90_000)],
        vec![Some(0), Some(90_001)],
        vec![None, Some(0), Some(180_000)],
        vec![Some(i64::MIN), Some(i64::MAX)],
    ];
    for left_times in &date_sets {
        for right_times in &date_sets {
            for distance in [0u32, 6, 7, 20, 21, 64] {
                let right_hash = if distance == 64 {
                    -1
                } else {
                    ((1u64 << distance) - 1) as i64
                };
                let mut left = SignatureSummary::new(0, left_times[0]);
                let mut right = SignatureSummary::new(right_hash, right_times[0]);
                for &time in &left_times[1..] {
                    left.add(time);
                }
                for &time in &right_times[1..] {
                    right.add(time);
                }
                let expected = left_times.iter().all(|a| {
                    right_times.iter().all(|b| {
                        let allowance = match (a, b) {
                            (Some(a), Some(b))
                                if (i128::from(*a) - i128::from(*b)).abs() <= 90_000 =>
                            {
                                20
                            }
                            _ => 6,
                        };
                        distance <= allowance
                    })
                });
                assert_eq!(
                    left.within_diameter(&right, 3, 10, 2, 90_000),
                    expected,
                    "distance {distance}, dates {left_times:?} / {right_times:?}"
                );
            }
        }
    }
}

#[test]
fn repeated_signatures_and_mixed_dates_preserve_every_output_pair_allowance() {
    let hashes = [0, 1023, 1_047_552, 0, 1023, 1_047_552, 7, 7];
    let times = [
        Some(90_000),
        Some(0),
        Some(180_000),
        None,
        Some(180_000),
        None,
        Some(0),
        Some(90_000),
    ];
    let groups = cluster_by_appearance(&hashes, &times, 3, 10, 90, 2, &|| false).unwrap();
    let mut actual_members = Vec::new();
    for group in &groups {
        actual_members.extend(group.iter().copied());
        for (position, &a) in group.iter().enumerate() {
            for &b in &group[position + 1..] {
                let allowance = match (times[a], times[b]) {
                    (Some(a), Some(b)) if (i128::from(a) - i128::from(b)).abs() <= 90_000 => 20,
                    _ => 6,
                };
                assert!(
                    (hashes[a] ^ hashes[b]).count_ones() <= allowance,
                    "invalid pair {a}, {b} in {group:?}"
                );
            }
        }
    }
    actual_members.sort_unstable();
    assert_eq!(actual_members, (0..hashes.len()).collect::<Vec<_>>());
    assert_eq!(
        groups,
        cluster_by_appearance(&hashes, &times, 3, 10, 90, 2, &|| false).unwrap()
    );
}

#[test]
fn latest_burst_signature_preserves_edges_after_older_duplicates_expire() {
    // The earlier 0 expires before the final 1023 arrives; its later twin
    // must remain in the window. Undated twins still join through strict edges.
    let hashes = [0, 0, 1023, 1023, 1_047_552, 0, 1023];
    let times = [
        Some(0),
        Some(80_000),
        Some(160_000),
        None,
        Some(240_000),
        Some(i64::MIN),
        Some(i64::MAX),
    ];
    let mut actual = UnionFind::new(hashes.len());
    let mut comparisons = 0;
    union_strict_neighbors(&hashes, 3, &mut actual, &mut comparisons, &|| false).unwrap();
    union_burst_neighbors(
        &hashes,
        &times,
        3,
        10,
        90_000,
        &mut actual,
        &mut comparisons,
        &|| false,
    )
    .unwrap();
    let mut expected = UnionFind::new(hashes.len());
    for a in 0..hashes.len() {
        for b in a + 1..hashes.len() {
            let allowance = match (times[a], times[b]) {
                (Some(a), Some(b)) if (i128::from(a) - i128::from(b)).abs() <= 90_000 => 10,
                _ => 3,
            };
            if (hashes[a] ^ hashes[b]).count_ones() <= allowance {
                expected.union(a, b);
            }
        }
    }
    for a in 0..hashes.len() {
        for b in 0..hashes.len() {
            assert_eq!(
                actual.find(a) == actual.find(b),
                expected.find(a) == expected.find(b),
                "connectivity differs for {a}, {b}"
            );
        }
    }
}

use onecopy_lib::work_priority::{Tier, Turns};
use onecopy_lib::{index_store, queries, resource_limits};

#[test]
fn continuous_library_turns_do_not_require_attention_or_inactivity() {
    let mut turns = Turns::default();
    for _ in 0..100 {
        let runnable = turns
            .order()
            .into_iter()
            .find(|tier| *tier == Tier::Library)
            .unwrap();
        turns.completed(runnable);
    }
    assert_eq!(turns.order()[0], Tier::VisibleRequired);
}

#[test]
fn fairness_never_displaces_urgent_preparation_but_bounds_section_monopoly() {
    let mut turns = Turns::default();
    for _ in 0..8 {
        turns.completed(Tier::SectionRequired);
    }
    assert_eq!(
        &turns.order()[..3],
        &[Tier::VisibleRequired, Tier::NearbyRequired, Tier::Library]
    );
    turns.completed(Tier::Library);
    assert_eq!(turns.order()[2], Tier::VisibleOptional);
}

#[test]
fn inference_reserves_cpu_headroom_even_without_user_input() {
    for (cpus, expected) in [(0, 1), (1, 1), (2, 1), (4, 2), (8, 4), (32, 4)] {
        assert_eq!(resource_limits::transcription_threads(cpus), expected);
    }
}

#[test]
fn outward_work_pages_follow_every_main_sort_and_survive_anchor_removal() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    for i in 0..180 {
        let hash = format!("hash-{i:03}");
        conn.execute("INSERT INTO contents(hash, byte_size, kind, width, height) VALUES (?1, ?2, 'image', ?3, ?4)",
            rusqlite::params![hash, i % 7, if i % 9 == 0 { None } else { Some(i % 5) }, i % 3]).unwrap();
        conn.execute("INSERT INTO paths(abs_path, dir_path, file_name, ext, kind, content_hash, resolved_utc_ms, resolved_source) VALUES (?1, '/', ?2, ?3, 'image', ?4, ?5, 'metadata')",
            rusqlite::params![format!("/{i}.jpg"), format!("{}-{}", if i % 2 == 0 { "A" } else { "a" }, i % 11), if i % 2 == 0 { "JPG" } else { "png" }, hash, 100 + i % 13]).unwrap();
    }
    let capabilities = onecopy_lib::derived_work::work_capabilities(root.path()).unwrap();
    for order in [
        queries::SectionSortOrder::Time,
        queries::SectionSortOrder::Name,
        queries::SectionSortOrder::Size,
        queries::SectionSortOrder::Resolution,
        queries::SectionSortOrder::Ext,
    ] {
        for desc in [false, true] {
            let sort = queries::SectionSort { order, desc };
            let expected = queries::section_window(
                &conn,
                "image",
                "1970-01",
                chrono_tz::UTC,
                sort,
                0,
                512,
                queries::ItemProjectionContext { capabilities },
            )
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.hash.unwrap())
            .collect::<Vec<_>>();
            let bounds = Some((0, 2_678_400_000));
            let anchor = queries::section_work_anchor(&conn, "image", bounds, sort, 80)
                .unwrap()
                .unwrap();
            assert_eq!(anchor.hash.as_ref(), Some(&expected[80]));
            for before in [false, true] {
                let mut cursor = anchor.clone();
                let mut actual = Vec::new();
                loop {
                    let page =
                        queries::section_work_page(&conn, "image", bounds, sort, &cursor, before)
                            .unwrap();
                    assert!(page.len() <= 64);
                    let Some(last) = page.last() else {
                        break;
                    };
                    cursor = last.clone();
                    actual.extend(page.into_iter().map(|row| row.hash.unwrap()));
                }
                let expected = if before {
                    expected[..80].iter().rev().cloned().collect::<Vec<_>>()
                } else {
                    expected[81..].to_vec()
                };
                assert_eq!(
                    actual, expected,
                    "{order:?} descending={desc} before={before}"
                );
            }
        }
    }
    let sort = queries::SectionSort {
        order: queries::SectionSortOrder::Name,
        desc: false,
    };
    let bounds = Some((0, 2_678_400_000));
    let anchor = queries::section_work_anchor(&conn, "image", bounds, sort, 80)
        .unwrap()
        .unwrap();
    let expected = queries::section_work_page(&conn, "image", bounds, sort, &anchor, false)
        .unwrap()
        .into_iter()
        .map(|row| row.hash)
        .collect::<Vec<_>>();
    conn.execute(
        "DELETE FROM paths WHERE content_hash = ?1",
        [anchor.hash.as_ref().unwrap()],
    )
    .unwrap();
    assert_eq!(
        queries::section_work_page(&conn, "image", bounds, sort, &anchor, false)
            .unwrap()
            .into_iter()
            .map(|row| row.hash)
            .collect::<Vec<_>>(),
        expected
    );

    conn.execute("UPDATE contents SET derived_at_utc = 'ready', derived_version = ?1", [onecopy_lib::preview::DERIVE_VERSION]).unwrap();
    conn.execute("UPDATE contents SET derived_at_utc = NULL WHERE hash IN ('hash-010', 'hash-170')", []).unwrap();
    let mut settings = onecopy_lib::derived_work::settings_from_config(None, root.path()).unwrap();
    settings.similarity_enabled = false;
    settings.face_enabled = false;
    let sort = queries::SectionSort { order: queries::SectionSortOrder::Time, desc: false };
    let section = onecopy_lib::derived_work::SectionPriority { kind: "image".to_string(), start_ms: Some(0), end_ms: Some(2_678_400_000) };
    let anchor = queries::section_work_anchor(&conn, "image", bounds, sort, 80).unwrap().unwrap();
    let mut pending = Vec::new();
    if anchor.hash.as_deref().is_some_and(|hash| hash == "hash-010" || hash == "hash-170") { pending.push(anchor.hash.clone().unwrap()); }
    for before in [true, false] {
        pending.extend(onecopy_lib::derived_work::section_pending_candidates(&conn, &settings, &section, sort, &anchor, before).unwrap().into_iter().filter_map(|row| row.hash));
    }
    pending.sort();
    assert_eq!(pending, ["hash-010", "hash-170"]);
}

#[test]
fn similarity_priority_does_not_consume_an_unrelated_earlier_cohort() {
    use onecopy_lib::similarity;
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    for (hash, timestamp) in [("old", 0_i64), ("visible", 2_678_400_000)] {
        conn.execute("INSERT INTO contents(hash, byte_size, kind, phash) VALUES (?1, 1, 'image', '0000000000000001')", [hash]).unwrap();
        conn.execute("INSERT INTO paths(abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source) VALUES (?1, '/', ?2, 'image', ?2, ?3, 'metadata')", rusqlite::params![format!("/{hash}.jpg"), hash, timestamp]).unwrap();
    }
    let settings = onecopy_lib::derived_work::settings_from_config(None, root.path()).unwrap();
    similarity::ensure_config_current(&conn, &settings.similarity).unwrap();
    let stats = similarity::rebuild_priority_bucket_cancellable(
        &conn,
        &settings.similarity,
        &["visible".to_string()],
        &|| false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(stats.last_bucket.as_deref(), Some("1970-02"));
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM similarity_dirty_buckets WHERE bucket = '1970-01'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert!(similarity::rebuild_priority_bucket_cancellable(
        &conn,
        &settings.similarity,
        &["visible".to_string()],
        &|| false
    )
    .unwrap()
    .is_none());
}

use super::*;

struct Fixture {
    conn: Connection,
    _directory: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
    Fixture { conn, _directory: directory }
}

fn insert(conn: &Connection, id: i64, size: i64, prehash: Option<&str>) {
    conn.execute(
        "INSERT INTO paths (id, abs_path, dir_path, file_name, size, kind, prehash)
         VALUES (?1, ?2, '/cohort', ?3, ?4, 'other', ?5)",
        params![id, format!("/cohort/{id}.bin"), format!("{id}.bin"), size, prehash],
    ).unwrap();
}

#[test]
fn index_connections_keep_temporary_cohorts_on_disk() {
    let fixture = fixture();
    assert_eq!(fixture.conn.pragma_query_value(None, "temp_store", |row| row.get::<_, i64>(0)).unwrap(), 1);
    let memory_only: bool = fixture.conn.query_row(
        "SELECT sqlite_compileoption_used('TEMP_STORE=3')", [], |row| row.get(0),
    ).unwrap();
    assert!(!memory_only, "SQLite must permit temporary tables to spill to disk");
}

#[test]
fn membership_pages_are_bounded_and_exclude_ineligible_paths() {
    let fixture = fixture();
    for id in 1..=265 { insert(&fixture.conn, id, 32, None); }
    fixture.conn.execute_batch(
        "UPDATE paths SET missing = 1 WHERE id = 261;
         UPDATE paths SET hash_attempt_failed = 1 WHERE id = 262;
         UPDATE paths SET size = 33 WHERE id = 263;
         INSERT INTO contents (hash, byte_size, kind) VALUES ('landed', 32, 'other');
         UPDATE paths SET content_hash = 'landed' WHERE id = 264;",
    ).unwrap();
    let cohort = Cohort::new(&fixture.conn).unwrap();
    cohort.select_size(32, 264).unwrap();
    assert_eq!(cohort.len().unwrap(), 260);
    let mut seen = Vec::new();
    let mut after = 0;
    loop {
        let rows = cohort.rows(after, None).unwrap();
        assert!(rows.len() <= 128);
        if rows.is_empty() { break; }
        assert!(rows.iter().all(|row| row.id > after));
        after = rows.last().unwrap().id;
        seen.extend(rows.into_iter().map(|row| row.id));
    }
    assert_eq!(seen, (1..=260).collect::<Vec<_>>());
}

#[test]
fn promoted_members_remain_available_with_their_current_path_facts() {
    let fixture = fixture();
    insert(&fixture.conn, 1, 32, Some("edge"));
    fixture.conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('p1', 32, 'other');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('landed', 32, 'other');
         UPDATE paths SET content_hash = 'p1' WHERE id = 1;",
    ).unwrap();
    let cohort = Cohort::new(&fixture.conn).unwrap();
    cohort.select_size(32, 1).unwrap();
    fixture.conn.execute(
        "UPDATE paths SET content_hash = 'landed', prehash = 'current' WHERE id = 1", [],
    ).unwrap();
    let rows = cohort.rows(0, Some("edge")).unwrap();
    assert_eq!(cohort.len().unwrap(), 1);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, 1);
    assert_eq!(row.abs, "/cohort/1.bin");
    assert_eq!(row.size, 32);
    assert_eq!(row.kind, "other");
    assert_eq!(row.provisional.as_deref(), Some("landed"));
    assert_eq!(row.prehash.as_deref(), Some("current"));
    assert!(cohort.rows(1, None).unwrap().is_empty());
}

#[test]
fn group_iteration_hash_counts_and_discard_use_the_snapshot() {
    let fixture = fixture();
    for id in 1..=132 { insert(&fixture.conn, id, 32, None); }
    let cohort = Cohort::new(&fixture.conn).unwrap();
    cohort.select_size(32, 132).unwrap();
    assert_eq!(cohort.next_prehash(None).unwrap(), None);
    for id in 1..=130 { cohort.set_prehash(id, "a").unwrap(); }
    cohort.set_prehash(131, "b").unwrap();
    assert_eq!(cohort.next_prehash(None).unwrap().as_deref(), Some("a"));
    assert_eq!(cohort.next_prehash(Some("a")).unwrap().as_deref(), Some("b"));
    assert_eq!(cohort.next_prehash(Some("b")).unwrap(), None);
    assert_eq!(cohort.group_len("a").unwrap(), 130);
    assert_eq!(cohort.rows(0, Some("a")).unwrap().len(), 128);
    let tail = cohort.rows(128, Some("a")).unwrap();
    assert_eq!(tail.iter().map(|row| row.id).collect::<Vec<_>>(), vec![129, 130]);
    assert_eq!(cohort.distinct_hashes("a").unwrap(), 0);
    cohort.set_hash(1, "first").unwrap();
    cohort.set_hash(2, "first").unwrap();
    cohort.set_hash(130, "second").unwrap();
    cohort.set_hash(131, "other-group").unwrap();
    assert_eq!(cohort.distinct_hashes("a").unwrap(), 2);
    cohort.discard(130).unwrap();
    cohort.discard(132).unwrap();
    assert_eq!(cohort.distinct_hashes("a").unwrap(), 1);
    assert_eq!(cohort.group_len("a").unwrap(), 129);
    assert_eq!(cohort.len().unwrap(), 130);
    assert!(cohort.rows(129, Some("a")).unwrap().is_empty());
    assert!(cohort.rows(0, Some("missing")).unwrap().is_empty());
}

#[test]
fn dropping_a_cohort_allows_a_fresh_membership_snapshot() {
    let fixture = fixture();
    insert(&fixture.conn, 1, 32, None);
    let cohort = Cohort::new(&fixture.conn).unwrap();
    cohort.select_size(32, 1).unwrap();
    assert_eq!(cohort.len().unwrap(), 1);
    insert(&fixture.conn, 2, 32, Some("later"));
    assert_eq!(cohort.len().unwrap(), 1);
    drop(cohort);
    let replacement = Cohort::new(&fixture.conn).unwrap();
    replacement.select_size(32, 2).unwrap();
    assert_eq!(replacement.len().unwrap(), 2);
    assert_eq!(replacement.next_prehash(None).unwrap().as_deref(), Some("later"));
}

#[test]
fn selecting_another_size_reuses_the_table_without_old_group_results() {
    let fixture = fixture();
    insert(&fixture.conn, 1, 32, Some("first"));
    insert(&fixture.conn, 2, 64, Some("second"));
    let cohort = Cohort::new(&fixture.conn).unwrap();
    cohort.select_size(32, 2).unwrap();
    cohort.set_hash(1, "landed").unwrap();
    assert_eq!(cohort.distinct_hashes("first").unwrap(), 1);
    cohort.select_size(64, 2).unwrap();
    assert_eq!(cohort.len().unwrap(), 1);
    assert_eq!(cohort.group_len("first").unwrap(), 0);
    assert_eq!(cohort.distinct_hashes("first").unwrap(), 0);
    assert_eq!(cohort.next_prehash(None).unwrap().as_deref(), Some("second"));
    let rows = cohort.rows(0, None).unwrap();
    assert_eq!(rows[0].id, 2);
}

#[test]
fn size_discovery_seeks_the_pending_and_established_indexes() {
    let fixture = fixture();
    let pending = size_query(true, crate::path_scope::Scope::Library);
    for (query, parameters, expected) in [
        (pending.as_str(), vec![32i64, 1000], "idx_paths_pending_size (size>?)"),
        (KNOWN_SIZE, vec![32i64], "idx_contents_established_size (byte_size=?)"),
    ] {
        let plan = fixture.conn.prepare(&format!("EXPLAIN QUERY PLAN {query}")).unwrap()
            .query_map(rusqlite::params_from_iter(parameters), |row| row.get::<_, String>(3)).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap().join("\n");
        assert!(plan.contains(expected), "{plan}");
    }
}

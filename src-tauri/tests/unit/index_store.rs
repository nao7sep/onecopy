use super::*;

#[test]
fn a_negative_index_is_rebuilt_but_a_newer_competing_setup_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite3");
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("CREATE TABLE obsolete(value); PRAGMA user_version = -1;").unwrap();
    drop(conn);
    let conn = open(&file).unwrap();
    assert_eq!(raw_user_version(&conn).unwrap(), crate::formats::INDEX);
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'obsolete'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    drop(conn);
    let competing = dir.path().join("competing.sqlite3");
    let result = open_before_setup(&competing, || {
        Connection::open(&competing).unwrap().execute_batch("CREATE TABLE future(value); PRAGMA user_version = 2;").unwrap();
    });
    assert!(result.unwrap_err().contains("newer OneCopy"));
    let conn = Connection::open(&competing).unwrap();
    assert_eq!(raw_user_version(&conn).unwrap(), 2);
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'contents'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn only_the_rebuildable_index_skips_waiting_for_the_disk() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(&dir.path().join("index.sqlite3")).unwrap();
    let synchronous = |schema: &str| -> i64 {
        conn.query_row(&format!("PRAGMA {schema}.synchronous"), [], |row| row.get(0)).unwrap()
    };
    assert_eq!(synchronous("main"), 1, "the index commits without waiting (NORMAL)");
    assert_eq!(synchronous("records"), 2, "the attached records keep FULL");
}

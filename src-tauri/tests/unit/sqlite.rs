use super::*;

#[test]
fn concurrent_setup_keeps_wal_and_leaves_readers_independent_of_writers() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("concurrent.sqlite3");
    let start = std::sync::Arc::new(std::sync::Barrier::new(8));
    let journal = std::sync::Arc::new(JournalSetup::new());
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            let start = start.clone();
            let journal = journal.clone();
            std::thread::spawn(move || {
                let conn = Connection::open(path).unwrap();
                start.wait();
                journal.configure(&conn, Duration::from_secs(5)).unwrap();
                assert_eq!(
                    conn.pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                        .unwrap(),
                    "wal"
                );
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }

    let writer = Connection::open(&path).unwrap();
    journal.configure(&writer, Duration::from_secs(5)).unwrap();
    writer.execute_batch("CREATE TABLE example(value INTEGER); INSERT INTO example VALUES(1); BEGIN IMMEDIATE; UPDATE example SET value = 2;").unwrap();
    let reader = Connection::open(&path).unwrap();
    journal
        .configure(&reader, Duration::from_millis(100))
        .unwrap();
    assert_eq!(
        reader
            .query_row("SELECT value FROM example", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    writer.execute_batch("COMMIT").unwrap();
    assert_eq!(
        reader
            .query_row("SELECT value FROM example", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
}

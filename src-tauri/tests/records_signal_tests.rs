// The stored-record signal the Records window's live updates follow: a commit
// that inserted a record signals once, whichever connection made it, and
// nothing else does. The listener is process-wide, so this file runs in a
// test target of its own, one step after another.

use std::sync::atomic::{AtomicUsize, Ordering};

use onecopy_lib::{index_store, records, records_view};

static SIGNALS: AtomicUsize = AtomicUsize::new(0);

fn signals() -> usize {
    SIGNALS.swap(0, Ordering::SeqCst)
}

#[test]
fn a_commit_that_stores_a_record_signals_once() {
    records::on_stored(|| {
        SIGNALS.fetch_add(1, Ordering::SeqCst);
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(records::RECORDS_DB_FILE_NAME);
    let writer = records::open(&path).unwrap();
    signals();

    let line = "INSERT INTO log_lines (session_id, time_utc, level, message, line) VALUES ('s', 't', 'info', 'm', '{}')";
    writer.execute(line, []).unwrap();
    assert_eq!(signals(), 1, "an inserted log line");

    let transaction = writer.unchecked_transaction().unwrap();
    transaction.execute(line, []).unwrap();
    transaction.execute(line, []).unwrap();
    transaction.commit().unwrap();
    assert_eq!(signals(), 1, "one transaction, two records");

    let transaction = writer.unchecked_transaction().unwrap();
    transaction.execute(line, []).unwrap();
    transaction.rollback().unwrap();
    writer.execute("UPDATE log_lines SET message = 'n'", []).unwrap();
    writer.execute("DELETE FROM log_lines WHERE id = 1", []).unwrap();
    assert_eq!(signals(), 0, "a rollback, an update and a delete store nothing");

    // An index connection writes Issues into the records it attaches.
    let index = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    signals();
    index
        .execute(
            "INSERT INTO records.issue_events (session_id, time_utc, kind, path, event) VALUES ('s', 't', 'k', '', 'occurred')",
            [],
        )
        .unwrap();
    assert_eq!(signals(), 1, "an Issue written through the index");
    index.execute("CREATE TEMP TABLE scratch (value)", []).unwrap();
    index.execute("INSERT INTO scratch VALUES (1)", []).unwrap();
    assert_eq!(signals(), 0, "a temp table is not the records");

    // The window's own reads store nothing, so they cannot start a read.
    let reader = records_view::open_reader(&path).unwrap();
    records_view::page(&reader, &records_view::RecordsQuery::default()).unwrap();
    records_view::sources(&reader, None).unwrap();
    assert_eq!(signals(), 0, "a read");
}

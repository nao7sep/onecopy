// The stored-record signal the Records window's live updates follow: it is
// sent after the write that stored a record has committed, so a read started
// by it already sees the record, whichever writer made it. The listener is
// process-wide, so this file runs in a test target of its own, one step after
// another.

use std::path::PathBuf;
use std::sync::Mutex;

use onecopy_lib::{index_store, records, records_view};

static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
/// What a reader saw at each signal: the log lines, Issue events and
/// activity events stored.
static SEEN: Mutex<Vec<(i64, i64, i64)>> = Mutex::new(Vec::new());

fn seen() -> Vec<(i64, i64, i64)> {
    std::mem::take(&mut *SEEN.lock().unwrap())
}

fn signals_with_activity(recorder: &onecopy_lib::activity::ActivityRecorder) {
    use onecopy_lib::activity::{ActivityDraft, ActivityKind, ActivityOwner};
    seen();
    recorder
        .record_at(ActivityDraft::new(ActivityOwner::App, ActivityKind::Admitted), "t".into(), 0)
        .unwrap();
    assert_eq!(seen(), vec![(3, 3, 1)], "an activity event");
}

fn readable(path: &std::path::Path) -> (i64, i64, i64) {
    let reader = records_view::open_reader(path).unwrap();
    let count = |table: &str| reader.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).unwrap();
    (count("log_lines"), count("issue_events"), count("activity_events"))
}

#[test]
fn a_record_is_signalled_once_it_is_readable() {
    records::on_stored(|| {
        let path = PATH.lock().unwrap().clone().unwrap();
        SEEN.lock().unwrap().push(readable(&path));
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(records::RECORDS_DB_FILE_NAME);
    *PATH.lock().unwrap() = Some(path.clone());
    let writer = records::open(&path).unwrap();
    let line = "INSERT INTO log_lines (session_id, time_utc, level, message, line) VALUES ('s', 't', 'info', 'm', '{}')";

    // A write outside a transaction has committed when it returns.
    writer.execute(line, []).unwrap();
    records::wrote(&writer);
    assert_eq!(seen(), vec![(1, 0, 0)], "an inserted log line");

    // Inside a transaction, nothing is signalled until the commit, and then
    // once, with both records readable.
    let transaction = writer.unchecked_transaction().unwrap();
    transaction.execute(line, []).unwrap();
    records::wrote(&transaction);
    transaction.execute(line, []).unwrap();
    records::wrote(&transaction);
    assert_eq!(seen(), vec![], "nothing before the commit");
    records::commit(transaction).unwrap();
    assert_eq!(seen(), vec![(3, 0, 0)], "one transaction, two records");

    // An Issue written through an index connection, outside and inside an
    // index transaction.
    let index = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    index_store::upsert_issue(&index, Some("/a.jpg"), "preview-failed", "no decoder").unwrap();
    assert_eq!(seen(), vec![(3, 1, 0)], "an Issue through the index");
    let transaction = index.unchecked_transaction().unwrap();
    index_store::upsert_issue(&transaction, Some("/b.jpg"), "preview-failed", "no decoder").unwrap();
    assert!(index_store::clear_issues(&transaction, "/a.jpg", &["preview-failed"]).unwrap());
    assert_eq!(seen(), vec![], "nothing before the index commit");
    records::commit(transaction).unwrap();
    assert_eq!(seen(), vec![(3, 3, 0)], "an index transaction's Issues");

    // The activity recorder's own connection.
    let recorder = onecopy_lib::activity::ActivityRecorder::new("s".into(), path.clone()).unwrap();
    signals_with_activity(&recorder);

    // A transaction that wrote no record signals nothing, and neither do the
    // window's own reads.
    let transaction = index.unchecked_transaction().unwrap();
    transaction.execute("CREATE TEMP TABLE scratch (value)", []).unwrap();
    records::commit(transaction).unwrap();
    let reader = records_view::open_reader(&path).unwrap();
    records_view::page(&reader, &records_view::RecordsQuery::default()).unwrap();
    records_view::sources(&reader, None).unwrap();
    assert_eq!(seen(), vec![], "no record");
}

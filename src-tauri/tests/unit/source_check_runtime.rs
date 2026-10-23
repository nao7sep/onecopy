use super::*;

#[test]
fn a_queued_source_check_uses_settings_committed_before_its_admission() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let _serial = crate::scan_runtime::serial_test();
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let old = root.path().join("old");
    let new = root.path().join("new");
    for path in [&data, &old, &new] { std::fs::create_dir(path).unwrap(); }
    std::fs::write(old.join("old.txt"), b"old").unwrap();
    std::fs::write(new.join("new.txt"), b"new").unwrap();
    crate::storage::save_config(&data, &json!({"sourceDirs": [old]})).unwrap();
    let foreground = crate::scan_runtime::admit_foreground(
        None, Some(Instant::now() + Duration::from_secs(5)), &|| false, &mut || {},
    ).unwrap();
    let (waiting_tx, waiting_rx) = mpsc::channel();
    let worker_data = data.clone();
    let worker = std::thread::spawn(move || {
        with_admitted_settings(&worker_data, move || {
            let _ = waiting_tx.send(());
            false
        }, |_| {}, |settings| {
            let conn = crate::index_store::open(&worker_data.join("index.sqlite3"))?;
            crate::scanner::run_source_check(&conn, settings, &|_| {})?;
            let mut statement = conn.prepare("SELECT file_name FROM paths ORDER BY file_name").unwrap();
            let names = statement.query_map([], |row| row.get::<_, String>(0)).unwrap()
                .collect::<rusqlite::Result<Vec<_>>>().unwrap();
            Ok(names)
        })
    });
    let waiting = waiting_rx.recv_timeout(Duration::from_secs(5));
    let saved = crate::storage::save_config(&data, &json!({"sourceDirs": [new]}));
    crate::scan_runtime::restart_source_walks();
    drop(foreground);
    let result = worker.join().unwrap();
    waiting.unwrap();
    saved.unwrap();
    assert_eq!(result.unwrap(), ["new.txt"]);
}

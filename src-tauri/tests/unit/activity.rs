use super::*;

#[test]
fn debug_diagnostics_do_not_create_ordinary_work_rows() {
    let temp = tempfile::tempdir().unwrap();
    let recorder =
        ActivityRecorder::new("one".into(), temp.path().join("activity.sqlite3")).unwrap();
    let draft = ActivityDraft::new(ActivityOwner::Selection, ActivityKind::Changed);
    recorder
        .record_with_visibility(draft, "2026-09-09T00:00:00.000Z".into(), 0, false)
        .unwrap();
    assert_eq!(recorder.page(None, 100).unwrap().0.len(), 1);
    assert!(recorder
        .operations(None, None, 100)
        .unwrap()
        .operations
        .is_empty());
}

#[test]
fn throttling_keeps_latest_counts_for_terminal_publication() {
    let trace = WorkTrace::begin(
        ActivityOwner::BackgroundWork,
        Some(ActivitySubject::Snapshots),
        None,
    );
    trace.progress(1, 10);
    let recorded = trace.progress.lock().unwrap().last_recorded;
    let report = trace.progress_reporter();
    report(10, 10);
    let progress = trace.progress.lock().unwrap();
    assert_eq!(progress.last_recorded, recorded);
    assert_eq!(progress.counts, Some((10, 10)));
}

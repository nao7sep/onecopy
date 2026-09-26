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

// (W-M3) Per-item background preview traces must never become ordinary
// Activity rows — the pass that owns their chunk already records one. This
// checks the routing `WorkTrace` itself carries; `debug_only` rows still go
// through the same `record()` debug gate as everything else.
#[test]
fn begin_debug_traces_are_marked_debug_only_not_ordinary() {
    let ordinary = WorkTrace::begin(ActivityOwner::BackgroundWork, None, None);
    assert_eq!(ordinary.visibility, TraceVisibility::Ordinary);

    let debug_only = WorkTrace::begin_debug(ActivityOwner::BackgroundWork, None, None);
    assert_eq!(debug_only.visibility, TraceVisibility::DebugOnly);
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

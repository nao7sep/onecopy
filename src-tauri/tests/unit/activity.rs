use super::*;

#[test]
fn records_details_retain_debug_diagnostic_visibility_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let recorder =
        ActivityRecorder::new("one".into(), temp.path().join("records.sqlite3")).unwrap();
    let draft = ActivityDraft::new(ActivityOwner::Selection, ActivityKind::Changed);
    recorder
        .record_with_visibility(draft, "2026-09-09T00:00:00.000Z".into(), 0, false)
        .unwrap();
    use crate::records_view::{self, RecordKind, RecordsQuery};
    let connection = records_view::open_reader(&temp.path().join("records.sqlite3")).unwrap();
    let page = records_view::page(&connection, &RecordsQuery {
        kind: Some(RecordKind::Activity), ..Default::default()
    }).unwrap();
    assert_eq!(page.records.len(), 1);
    let detail = records_view::detail(&connection, RecordKind::Activity, page.records[0].id).unwrap().unwrap();
    let visible = &detail.fields.iter().find(|field| field.name == "user_visible").unwrap().value;
    assert_eq!(visible, &serde_json::json!(0));
}

// Per-item background preview traces must never become ordinary
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

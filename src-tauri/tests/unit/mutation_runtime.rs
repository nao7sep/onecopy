use super::*;

#[test]
fn terminal_classification_preserves_failure_unknown_cancellation_and_no_receipt() {
    use crate::activity::ActivityState;
    let complete = result_summary(2, 2, 2, 2, 2, 0, 0, false, None);
    let partial = ResultSummary { files_failed: 1, ..complete.clone() };
    let unknown = ResultSummary { files_unknown: 1, ..complete.clone() };
    let stopped = ResultSummary { error: Some("index unavailable".into()), ..complete.clone() };
    for (cancelled, failed, summary, expected, wire) in [
        (false, false, Some(&complete), ActivityState::Succeeded, "succeeded"),
        (false, false, Some(&partial), ActivityState::Failed, "failed"),
        (false, false, Some(&unknown), ActivityState::OutcomeUnknown, "outcomeUnknown"),
        (false, true, Some(&unknown), ActivityState::OutcomeUnknown, "outcomeUnknown"),
        (false, false, Some(&stopped), ActivityState::Failed, "failed"),
        (false, true, Some(&complete), ActivityState::Failed, "failed"),
        (true, false, Some(&partial), ActivityState::Cancelled, "cancelled"),
        (true, false, None, ActivityState::Cancelled, "cancelled"),
        (false, false, None, ActivityState::Idle, "idle"),
    ] {
        let outcome = terminal_state(cancelled, failed, summary);
        assert_eq!(outcome, expected);
        assert_eq!(serde_json::to_value(outcome).unwrap(), json!(wire));
    }
}

fn volume_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Vec<std::path::PathBuf>,
) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("apphome");
    let roots = ["original", "healthy", "destination"]
        .map(|name| dir.path().join(name))
        .to_vec();
    for root in roots.iter().chain(std::iter::once(&data)) {
        std::fs::create_dir_all(root).unwrap();
    }
    std::fs::write(
        data.join(crate::storage::CONFIG_FILE_NAME),
        json!({
            "formatVersion": 1,
            "sourceDirs": [&roots[0], &roots[1]],
            "destinationRoots": [&roots[2], roots[0].join("nested")],
        })
        .to_string(),
    )
    .unwrap();
    crate::volume::check_identity(&data, &roots[0].to_string_lossy(), "volume-a").unwrap();
    crate::volume::check_identity(&data, &roots[1].to_string_lossy(), "volume-b").unwrap();
    (dir, data, roots)
}

#[test]
fn empty_checks_the_configured_source_owner_before_any_removal() {
    let (_dir, data, roots) = volume_fixture();
    let gate = |data: &std::path::Path, sources: &[String]| {
        crate::volume::enforce_no_substitution_with(data, sources, &|_| Some("volume-b".into()))
    };
    for owner in [&roots[0], &roots[0].join("nested")] {
        let day = owner
            .join(crate::trash::TRASH_DIR_NAME)
            .join("20261011-utc");
        std::fs::create_dir_all(&day).unwrap();
        let file = day.join("photo.jpg");
        std::fs::write(&file, b"keep").unwrap();
        let reviewed = crate::trash::overview(std::slice::from_ref(owner)).remove(0);
        let result = empty_trash_gate_with(&data, std::path::Path::new(&reviewed.root), &gate)
            .and_then(|()| {
                crate::trash::empty_root_with_progress(
                    std::path::Path::new(&reviewed.root),
                    &reviewed.plan_token,
                    &AtomicBool::new(false),
                    &|_| {},
                    &|_, _| Ok(()),
                )
            });
        assert!(
            result.is_err(),
            "a fresh token does not authorize a substituted source"
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"keep");
    }
    for owner in &roots[1..] {
        let location = owner.join(crate::trash::TRASH_DIR_NAME);
        assert!(
            empty_trash_gate_with(&data, &location, &gate).is_ok(),
            "unaffected sources and destinations stay usable"
        );
    }
}

#[test]
fn volume_admission_rechecks_after_background_work_and_releases_its_guard() {
    let _serial = crate::scan_runtime::serial_test();
    let (_dir, data, roots) = volume_fixture();
    let current = std::sync::Arc::new(std::sync::Mutex::new("volume-a"));
    let claim = begin().unwrap();
    let touched = vec![roots[0].to_string_lossy().into_owned()];
    let (result, waited) = std::thread::scope(|scope| {
        let (release, released) = std::sync::mpsc::channel();
        let (held, holding) = std::sync::mpsc::channel();
        let background = scope.spawn(move || {
            crate::scan_runtime::with_owner(
                crate::scan_runtime::Owner::Watcher,
                || false,
                || {
                    held.send(()).unwrap();
                    let _ = released.recv();
                    Ok(())
                },
            )
        });
        holding.recv().unwrap();
        let (waiting, observed) = std::sync::mpsc::channel();
        let current = &current;
        let data = &data;
        let claim = &claim;
        let admitted = scope.spawn(move || {
            admit_checked(
                data,
                &touched,
                &|data, sources| {
                    crate::volume::enforce_no_substitution_with(data, sources, &|_| {
                        Some(current.lock().unwrap().to_string())
                    })
                },
                || match crate::scan_runtime::admit_foreground(
                    None,
                    None,
                    &|| claim.cancelled(),
                    &mut || {
                        waiting.send(()).unwrap();
                    },
                ) {
                    Ok(guard) => Ok(Some(guard)),
                    Err(crate::scan_runtime::Refusal::Cancelled) => Ok(None),
                    Err(error) => Err(format!("{error:?}")),
                },
            )
            .map(|guard| guard.is_some())
        });
        let waited = observed.recv_timeout(Duration::from_secs(5)).is_ok();
        *current.lock().unwrap() = "volume-b";
        release.send(()).unwrap();
        let result = admitted.join().unwrap();
        background.join().unwrap().unwrap();
        (result, waited)
    });
    assert!(waited, "the real index owner was awaited");
    assert!(
        result.is_err(),
        "a source substituted during the wait is refused"
    );
    // A refused late check releases its index reservation.
    assert!(crate::scan_runtime::admit_foreground(
        None,
        Some(Instant::now() + Duration::from_secs(1)),
        &|| false,
        &mut || {}
    )
    .is_ok());
    let healthy = vec![roots[1].to_string_lossy().into_owned()];
    assert_eq!(
        admit_checked(
            &data,
            &healthy,
            &|data, sources| {
                crate::volume::enforce_no_substitution_with(data, sources, &|_| {
                    Some("volume-b".into())
                })
            },
            || Ok(Some(()))
        ),
        Ok(Some(())),
        "unaffected source remains admitted"
    );
}

#[cfg(unix)]
#[test]
fn canonical_accepted_paths_keep_their_configured_alias_volume_gate() {
    let (_dir, data, roots) = volume_fixture();
    let alias = roots[0].with_file_name("configured-alias");
    std::os::unix::fs::symlink(&roots[0], &alias).unwrap();
    std::fs::write(
        data.join(crate::storage::CONFIG_FILE_NAME),
        json!({
            "formatVersion": 1, "sourceDirs": [&alias, &roots[1]],
        })
        .to_string(),
    )
    .unwrap();
    crate::volume::check_identity(&data, &alias.to_string_lossy(), "volume-a").unwrap();
    let resolved = std::fs::canonicalize(&roots[0]).unwrap();
    let photo = resolved.join("photo.jpg");
    std::fs::write(&photo, b"keep").unwrap();
    let conn = crate::index_store::open(&data.join(crate::storage::INDEX_DB_FILE_NAME)).unwrap();
    conn.execute(
        "INSERT INTO scan_dirs (root, dirty, configured_root) VALUES (?1, 0, ?2)",
        rusqlite::params![resolved.to_string_lossy(), alias.to_string_lossy()],
    )
    .unwrap();
    // Scanner's canonical row remains accepted after the configured alias
    // points elsewhere. Admission must still use that alias's recorded A.
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&roots[1], &alias).unwrap();
    let accepted = crate::operations::AcceptedFiles::for_test_paths([photo.to_string_lossy()]);
    let sources = crate::storage::configured_source_dirs(&data).unwrap();
    let unrelated =
        crate::volume_io::FakeStallingVolume::mount(&roots[1], Duration::from_millis(100));
    unrelated.stall(&[], None);
    let touched = touched_source_dirs_at(&data, &sources, &Touches::Files(&accepted)).unwrap();
    assert_eq!(touched, [alias.to_string_lossy().into_owned()]);
    assert_eq!(
        unrelated.held(),
        0,
        "mapping modern receipts probes no unrelated source"
    );
    unrelated.release();
    let result = admit_checked(
        &data,
        &touched,
        &|data, sources| {
            crate::volume::enforce_no_substitution_with(data, sources, &|_| Some("volume-b".into()))
        },
        || {
            std::fs::remove_file(&photo).unwrap();
            Ok(Some(()))
        },
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read(&photo).unwrap(), b"keep");
    let healthy = crate::operations::AcceptedFiles::for_test_paths([roots[1]
        .join("photo.jpg")
        .to_string_lossy()]);
    assert_eq!(
        touched_source_dirs_at(&data, &sources, &Touches::Files(&healthy)).unwrap(),
        [roots[1].to_string_lossy().into_owned()],
        "no blanket gate on another source"
    );
}

#[test]
fn obsolete_null_source_receipts_and_read_failures_close_admission() {
    let (_dir, data, roots) = volume_fixture();
    let sources: Vec<_> = roots
        .iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect();
    let accepted = crate::operations::AcceptedFiles::for_test_paths([roots[0]
        .join("photo.jpg")
        .to_string_lossy()]);
    let index = data.join(crate::storage::INDEX_DB_FILE_NAME);
    let conn = crate::index_store::open(&index).unwrap();
    // Simulate an obsolete receipt shape without passing through the owning
    // startup opener, which rebuilds this disposable index before normal use.
    conn.execute_batch(
        "DROP TABLE scan_dirs; CREATE TABLE scan_dirs(root TEXT, configured_root TEXT);",
    )
    .unwrap();
    conn.execute("INSERT INTO scan_dirs(root) VALUES (?1)", [&sources[0]])
        .unwrap();
    assert!(touched_source_dirs_at(&data, &sources, &Touches::Files(&accepted)).is_err());
    drop(conn);
    std::fs::write(&index, b"unreadable source receipts").unwrap();
    assert!(
        touched_source_dirs_at(&data, &sources, &Touches::Files(&accepted)).is_err(),
        "reading no receipts is distinct from a failed receipt read"
    );
}

#[test]
fn early_volume_refusal_and_cancelled_wait_do_not_run_or_repeat_the_effect() {
    let waited = std::cell::Cell::new(false);
    let refused: Result<Option<()>, _> = admit_checked(
        std::path::Path::new("/apphome"),
        &[],
        &|_, _| Err("source changed".into()),
        || {
            waited.set(true);
            Ok(Some(()))
        },
    );
    assert_eq!(refused, Err("source changed".into()));
    assert!(!waited.get());
    let probes = std::cell::Cell::new(0);
    let cancelled: Result<Option<()>, _> = admit_checked(
        std::path::Path::new("/apphome"),
        &[],
        &|_, _| {
            probes.set(probes.get() + 1);
            Ok(())
        },
        || Ok(None),
    );
    assert_eq!(cancelled, Ok(None));
    assert_eq!(probes.get(), 1, "cancelled admission stays unstarted");
}

#[cfg(unix)]
#[test]
fn empty_and_restore_root_scope_find_a_physical_source_alias_without_index_receipts() {
    let (_dir, data, roots) = volume_fixture();
    let alias = roots[0].with_file_name("source-alias");
    std::os::unix::fs::symlink(&roots[0], &alias).unwrap();
    let nested = std::fs::canonicalize(&roots[0]).unwrap().join("nested");
    let day = nested
        .join(crate::trash::TRASH_DIR_NAME)
        .join("20261011-utc");
    std::fs::create_dir_all(&day).unwrap();
    let photo = day.join("photo.jpg");
    std::fs::write(&photo, b"keep").unwrap();
    std::fs::write(
        data.join(crate::storage::CONFIG_FILE_NAME),
        json!({
            "formatVersion": 1, "sourceDirs": [&alias, &roots[1]],
            "destinationRoots": [&nested, &roots[2]],
        })
        .to_string(),
    )
    .unwrap();
    crate::volume::check_identity(&data, &alias.to_string_lossy(), "volume-a").unwrap();
    let sources = crate::storage::configured_source_dirs(&data).unwrap();
    assert!(!data.join(crate::storage::INDEX_DB_FILE_NAME).exists());
    assert_eq!(
        touched_source_dirs_at(&data, &sources, &Touches::Root(&nested.to_string_lossy())).unwrap(),
        [alias.to_string_lossy().into_owned()],
        "Restore's touched root has the same physical owner"
    );
    let reviewed = crate::trash::overview(std::slice::from_ref(&nested)).remove(0);
    let result = empty_trash_gate_with(
        &data,
        std::path::Path::new(&reviewed.root),
        &|data, sources| {
            crate::volume::enforce_no_substitution_with(data, sources, &|_| Some("volume-b".into()))
        },
    )
    .and_then(|()| {
        crate::trash::empty_root_with_progress(
            std::path::Path::new(&reviewed.root),
            &reviewed.plan_token,
            &AtomicBool::new(false),
            &|_| {},
            &|_, _| Ok(()),
        )
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(&photo).unwrap(), b"keep");
    assert!(
        !data.join(crate::storage::INDEX_DB_FILE_NAME).exists(),
        "Empty creates no index"
    );
    // Existing index without a scan receipt has the same absence semantics.
    let conn = crate::index_store::open(&data.join(crate::storage::INDEX_DB_FILE_NAME)).unwrap();
    assert_eq!(
        touched_source_dirs_at(&data, &sources, &Touches::Root(&nested.to_string_lossy())).unwrap(),
        [alias.to_string_lossy().into_owned()]
    );
    drop(conn);
    let unrelated =
        crate::volume_io::FakeStallingVolume::mount(&roots[1], Duration::from_millis(100));
    unrelated.stall(&[crate::volume_io::Op::Stat], None);
    assert!(
        empty_trash_gate_with(
            &data,
            &roots[2].join(crate::trash::TRASH_DIR_NAME),
            &|data, sources| {
                crate::volume::enforce_no_substitution_with(data, sources, &|_| {
                    Some("volume-b".into())
                })
            },
        )
        .is_ok(),
        "failed resolution of an unrelated source never gates a destination"
    );
    unrelated.release();
    assert!(unrelated.wait_until_settled(Duration::from_secs(2)));
}

#[test]
fn remembered_source_owner_uses_the_exact_current_volume_record_key() {
    let data = tempfile::tempdir().unwrap();
    let conn =
        crate::index_store::open(&data.path().join(crate::storage::INDEX_DB_FILE_NAME)).unwrap();
    let sources = vec![
        "/configured/Photos".to_string(),
        "/configured/photos".to_string(),
    ];
    conn.execute(
        "INSERT INTO scan_dirs (root, dirty, configured_root) VALUES (?1, 0, ?2)",
        rusqlite::params!["/settled", &sources[0]],
    )
    .unwrap();
    let accepted = crate::operations::AcceptedFiles::for_test_paths(["/settled/photo.jpg"]);
    assert_eq!(
        touched_source_dirs_at(data.path(), &sources, &Touches::Files(&accepted)).unwrap(),
        [sources[0].clone()],
        "record keys must not inherit containment's spelling normalization"
    );
}

#[test]
fn cancellation_is_bound_to_one_claim_identity() {
    let _serial = crate::scan_runtime::serial_test();
    let first = begin().unwrap();
    let first_id = first.id();
    assert!(begin().is_err());
    assert!(request_cancel(first_id).unwrap());
    assert!(first.cancelled());
    drop(first);
    assert!(!request_active_cancel().unwrap());

    let second = begin().unwrap();
    assert_ne!(second.id(), first_id);
    assert!(!request_cancel(first_id).unwrap());
    assert!(!second.cancelled());
    assert!(request_active_cancel().unwrap());
    assert!(second.cancelled());
}

#[test]
fn result_accounting_separates_complete_partial_and_unstarted_work() {
    assert_eq!(
        result_summary(8, 4, 3, 12, 7, 2, 0, true, None),
        ResultSummary {
            items_completed: 3,
            items_partial: 1,
            items_unstarted: 4,
            files_completed: 5,
            files_failed: 2,
            files_unknown: 0,
            files_unstarted: 5,
            files_already_present: 0,
            trash_available: true,
            error: None,
        }
    );
}

// A file whose rename or removal was given up on is neither completed nor a
// known failure: it is reported on its own.
#[test]
fn result_accounting_reports_unknown_outcomes_apart_from_failures() {
    assert_eq!(
        result_summary(2, 2, 1, 4, 4, 3, 2, false, None),
        ResultSummary {
            items_completed: 1,
            items_partial: 1,
            items_unstarted: 0,
            files_completed: 1,
            files_failed: 1,
            files_unknown: 2,
            files_unstarted: 0,
            files_already_present: 0,
            trash_available: false,
            error: None,
        }
    );
}

#[test]
fn section_recheck_answers_busy_while_a_file_operation_runs() {
    let _serial = crate::scan_runtime::serial_test();
    let operation = begin().unwrap();
    assert!(crate::scan_runtime::section_admission().is_err());
    drop(operation);
    assert!(crate::scan_runtime::section_admission().is_ok());
}

// Quitting must not wait past its deadline for a file operation that never
// reaches its own safe point: it gives up on the current file, not on the whole
// exit sequence.
#[test]
fn wait_for_idle_gives_up_at_its_deadline_when_the_claim_never_drops() {
    let _serial = crate::scan_runtime::serial_test();
    let claim = begin().unwrap();
    let started = std::time::Instant::now();
    let outcome = wait_for_idle(std::time::Duration::from_millis(50));
    assert_eq!(outcome, Ok(IdleWait::TimedOut));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "must give up at the deadline, not hang"
    );
    drop(claim);
}

// A file operation that reaches its safe point (drops its claim) before the
// deadline lets exit proceed at once, without waiting out the full deadline.
#[test]
fn wait_for_idle_returns_as_soon_as_the_claim_drops_before_the_deadline() {
    let _serial = crate::scan_runtime::serial_test();
    let claim = begin().unwrap();
    let released = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(claim);
    });
    let started = std::time::Instant::now();
    let outcome = wait_for_idle(std::time::Duration::from_secs(10));
    released.join().unwrap();
    assert_eq!(outcome, Ok(IdleWait::Idle));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "must not wait out the full deadline once the claim already dropped"
    );
}

// The volume-substitution gate at mutation admission is scoped
// to the configured roots the accepted batch actually sits under, so a
// substituted or unverifiable source never blocks a batch that touches only
// another, unaffected root.
#[test]
fn touched_source_dirs_scopes_to_roots_the_accepted_batch_actually_sits_under() {
    let source_dirs = vec![
        "/Volumes/A".to_string(),
        "/Volumes/B".to_string(),
        "/Volumes/C".to_string(),
    ];
    let accepted = crate::operations::AcceptedFiles::for_test_paths([
        "/Volumes/A/2024/IMG_0001.jpg",
        "/Volumes/A/2024/IMG_0001.xmp",
    ]);
    let touched = touched_source_dirs(&source_dirs, &Touches::Files(&accepted));
    assert_eq!(touched, vec!["/Volumes/A".to_string()]);
}

#[test]
fn touched_source_dirs_is_empty_when_the_batch_touches_no_configured_source() {
    let source_dirs = vec!["/Volumes/A".to_string()];
    let accepted =
        crate::operations::AcceptedFiles::for_test_paths(["/Volumes/Elsewhere/photo.jpg"]);
    assert!(touched_source_dirs(&source_dirs, &Touches::Files(&accepted)).is_empty());
}

// A restore's receipt keeps "already there" apart from failures and from
// outcome-unknown renames.
#[test]
fn a_restore_receipt_counts_restored_already_there_failed_unknown_and_unstarted() {
    let outcome = crate::restore::RestoreOutcome {
        restored: vec!["/r/a.jpg".to_string(), "/r/b.jpg".to_string()],
        already_present: 1,
        failed: 2,
        unknown: 1,
        unstarted: 3,
        files_total: 8,
        ..Default::default()
    };
    assert_eq!(
        restore_summary(&outcome),
        ResultSummary {
            items_completed: 2,
            items_partial: 0,
            items_unstarted: 3,
            files_completed: 2,
            files_failed: 1,
            files_unknown: 1,
            files_unstarted: 3,
            files_already_present: 1,
            trash_available: false,
            error: None,
        }
    );
}

// Restore touches one configured root: a source containing it is gated, a
// destination root never is (the manifest travels with its own drive).
#[test]
fn a_restore_gates_only_the_source_its_root_lies_in() {
    let source_dirs = vec!["/Volumes/A".to_string(), "/Volumes/B".to_string()];
    assert_eq!(
        touched_source_dirs(&source_dirs, &Touches::Root("/Volumes/B")),
        vec!["/Volumes/B".to_string()]
    );
    assert_eq!(
        touched_source_dirs(&source_dirs, &Touches::Root("/Volumes/A/nested")),
        vec!["/Volumes/A".to_string()]
    );
    assert!(touched_source_dirs(&source_dirs, &Touches::Root("/Volumes/Backup")).is_empty());
}

#[test]
fn a_rebuild_requested_during_a_file_operation_is_refused_at_once_and_never_queued() {
    // `begin_rebuild` takes this same claim, reporting only state failures.
    let _serial = crate::scan_runtime::serial_test();
    let operation = begin().unwrap();
    let requested = std::time::Instant::now();
    assert!(begin().is_err());
    assert!(requested.elapsed() < std::time::Duration::from_millis(100));
    drop(operation);
    // Nothing waited to run once the operation ended.
    assert!(!active());
    // The running operation was the reason: a new request is admitted now.
    assert!(begin().is_ok());
}

// ---------------------------------------------------------------------------
// Restore through its orchestration, with a host standing in for the app

struct TestRestoreHost {
    data_root: std::path::PathBuf,
    waiting_shown: std::sync::Arc<AtomicBool>,
    phases: Vec<Phase>,
    finished: Vec<(bool, Option<ResultSummary>)>,
    sections: Vec<Option<Vec<crate::queries::SectionLocation>>>,
    media: Vec<Option<Vec<String>>>,
}

impl RestoreHost for TestRestoreHost {
    type Admitted = crate::scan_runtime::ForegroundGuard;

    fn data_root(&self) -> Result<std::path::PathBuf, String> {
        Ok(self.data_root.clone())
    }

    fn admit(
        &mut self,
        mutation: &Claim,
        _root: &str,
        media: Option<&[String]>,
        waiting: &Progress,
    ) -> Result<Option<Self::Admitted>, String> {
        self.media.push(media.map(<[String]>::to_vec));
        let (phases, shown) = (&mut self.phases, &self.waiting_shown);
        match crate::scan_runtime::admit_foreground(
            None,
            None,
            &|| mutation.cancelled(),
            &mut || {
                phases.push(waiting.phase);
                shown.store(true, Ordering::SeqCst);
            },
        ) {
            Ok(guard) => Ok(Some(guard)),
            Err(crate::scan_runtime::Refusal::Cancelled) => Ok(None),
            Err(refusal) => Err(format!("{refusal:?}")),
        }
    }

    fn progress(&mut self, progress: &Progress) {
        self.phases.push(progress.phase);
    }

    fn done(
        &mut self,
        _progress: &Progress,
        cancelled: bool,
        summary: Option<ResultSummary>,
        sections: Option<Vec<crate::queries::SectionLocation>>,
    ) {
        self.finished.push((cancelled, summary));
        self.sections.push(sections);
    }

    fn error(
        &mut self,
        _progress: &Progress,
        error: &str,
        _sections: Option<Vec<crate::queries::SectionLocation>>,
    ) {
        panic!("the restore failed: {error}");
    }

    fn information_owed(&mut self) {}
}

/// One source root with one deleted file whose folder is gone, and the
/// restore's data folder. Returns (dir, root, data root, stored path, id).
fn restore_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    String,
    String,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("photos");
    let data_root = dir.path().join("apphome");
    std::fs::create_dir_all(root.join("trip")).unwrap();
    std::fs::create_dir_all(&data_root).unwrap();
    std::fs::write(
        data_root.join(crate::storage::CONFIG_FILE_NAME),
        json!({ "formatVersion": 1, "sourceDirs": [root.to_string_lossy()] }).to_string(),
    )
    .unwrap();
    let original = root.join("trip").join("a.jpg");
    std::fs::write(&original, b"bytes").unwrap();
    let record = crate::trash::trash_file(
        &original,
        &root,
        None,
        &crate::trash::TrashContext::new(crate::trash::TrashKind::Delete, "op"),
    )
    .unwrap();
    std::fs::remove_dir(root.join("trip")).unwrap();
    let stored = std::path::Path::new(&record.stored_path);
    let id = format!(
        "{}/{}",
        stored
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy(),
        record.stored_name
    );
    (dir, root, data_root, record.stored_path, id)
}

/// Runs a restore of `id` while background work holds the index; once the
/// restore shows that it waits, `while_waiting` runs and then the background
/// work finishes.
fn restore_behind_background_work(
    root: &std::path::Path,
    host: &mut TestRestoreHost,
    id: String,
    while_waiting: impl FnOnce(),
) -> crate::restore::RestoreOutcome {
    let mutation = begin().unwrap();
    let shown = host.waiting_shown.clone();
    let outcome = std::thread::scope(|scope| {
        // Owned by this closure, so a failed assertion below drops the sender
        // while unwinding and the background work ends; the scope can then
        // join it and report the failure instead of waiting forever.
        let (release, released) = std::sync::mpsc::channel::<()>();
        // Background work holds the index and has not reached a safe point.
        let (held, holding) = std::sync::mpsc::channel::<()>();
        let background = scope.spawn(move || {
            crate::scan_runtime::with_owner(
                crate::scan_runtime::Owner::Watcher,
                || false,
                || {
                    held.send(()).unwrap();
                    // Released by the test, or by the test failing.
                    let _ = released.recv();
                    Ok(())
                },
            )
        });
        holding.recv().unwrap();
        let location = root
            .join(crate::trash::TRASH_DIR_NAME)
            .to_string_lossy()
            .into_owned();
        let mutation = &mutation;
        let restore =
            scope.spawn(move || restore_claimed(mutation, host, location, vec![id], None));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !shown.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "the restore never showed that it waits"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        while_waiting();
        release.send(()).unwrap();
        let outcome = restore.join().unwrap().unwrap();
        background.join().unwrap().unwrap();
        outcome
    });
    drop(mutation);
    outcome
}

fn test_host(data_root: &std::path::Path) -> TestRestoreHost {
    TestRestoreHost {
        data_root: data_root.to_path_buf(),
        waiting_shown: Default::default(),
        phases: Vec::new(),
        finished: Vec::new(),
        sections: Vec::new(),
        media: Vec::new(),
    }
}

// Cancel while the restore waits for background work ends
// the wait, and nothing is observed, created, moved or recorded.
#[test]
fn cancelling_a_restore_while_it_waits_for_background_work_changes_nothing() {
    let _serial = crate::scan_runtime::serial_test();
    let (_dir, root, data_root, stored, id) = restore_fixture();
    let manifest = std::path::Path::new(&stored)
        .parent()
        .unwrap()
        .join(crate::trash::MANIFEST_FILE_NAME);
    let manifest_before = std::fs::read(&manifest).unwrap();
    let mut host = test_host(&data_root);

    let outcome = restore_behind_background_work(&root, &mut host, id, || {
        assert!(request_active_cancel().unwrap());
    });

    assert!(outcome.cancelled);
    assert_eq!(
        (outcome.restored.len(), outcome.unstarted, outcome.failed),
        (0, 1, 0)
    );
    assert_eq!(
        host.phases,
        [Phase::Waiting],
        "never planned, never restored"
    );
    assert_eq!(host.finished.len(), 1);
    let (cancelled, summary) = host.finished[0].clone();
    assert!(cancelled);
    assert_eq!(summary.map(|summary| summary.files_unstarted), Some(1));
    assert_eq!(host.sections, [Some(Vec::new())], "no section changed");
    assert!(
        std::path::Path::new(&stored).exists(),
        "still in Deleted files"
    );
    assert!(!root.join("trip").exists(), "no folder recreated");
    assert_eq!(
        std::fs::read(&manifest).unwrap(),
        manifest_before,
        "no restored line"
    );
    let conn =
        crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME)).unwrap();
    let issues: i64 = conn
        .query_row("SELECT COUNT(*) FROM active_issues", [], |row| row.get(0))
        .unwrap();
    assert_eq!(issues, 0, "no Issue recorded");
}

// The same wait without Cancel: the restore runs once background work
// reaches its safe point.
#[test]
fn a_restore_that_waited_for_background_work_runs_once_it_is_admitted() {
    let _serial = crate::scan_runtime::serial_test();
    let (_dir, root, data_root, stored, id) = restore_fixture();
    let mut host = test_host(&data_root);

    let outcome = restore_behind_background_work(&root, &mut host, id, || {});

    assert!(!outcome.cancelled);
    assert!(
        outcome.requires_review,
        "the recreated folder is reviewed first"
    );
    let token = outcome.plan_token.clone();
    let location = root
        .join(crate::trash::TRASH_DIR_NAME)
        .to_string_lossy()
        .into_owned();
    let mutation = begin().unwrap();
    let ids = vec![outcome.review.unwrap().files[0].id.clone()];
    let confirmed = restore_claimed(&mutation, &mut host, location, ids, token).unwrap();
    drop(mutation);
    assert_eq!(confirmed.restored.len(), 1);
    assert!(!std::path::Path::new(&stored).exists());
    // Restore changes no indexed file: no reader is paused and no derived
    // job, a requested one included, is stopped for it (an empty key list
    // would stop every item's).
    assert_eq!(host.media, [None, None]);
    assert_eq!(
        std::fs::read(root.join("trip").join("a.jpg")).unwrap(),
        b"bytes"
    );
}

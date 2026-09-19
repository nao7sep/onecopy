use super::*;
use std::cell::{Cell, RefCell};

#[test]
fn invalid_index_path_never_yields_worker_admission_state() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join(crate::storage::INDEX_DB_FILE_NAME)).unwrap();

    let error = match prepare_data(temp.path()) {
        Ok(_) => panic!("invalid index path unexpectedly prepared startup data"),
        Err(error) => error,
    };

    assert!(!error.is_empty());
    assert!(!temp.path().join(crate::storage::CACHE_DIR_NAME).exists());
}

#[test]
fn blocked_gate_exposes_only_stable_private_safe_copy() {
    let failure = StartupGate::blocked().failure().unwrap();
    assert_eq!(failure.title, "OneCopy could not start safely");
    assert!(failure.message.contains("Your photos were not changed"));
    assert!(!failure.message.contains('/'));
    assert!(!failure.message.contains("sqlite"));
}

#[test]
fn runtime_service_failures_do_not_skip_later_admission() {
    let later_started = Cell::new(false);
    let failures = RefCell::new(Vec::new());
    let services = vec![
        RuntimeService {
            name: "returned failure",
            issue_kind: "returned-failure",
            start: Box::new(|| Err("could not start".to_string())),
        },
        RuntimeService {
            name: "panic failure",
            issue_kind: "panic-failure",
            start: Box::new(|| panic!("service panic")),
        },
        RuntimeService {
            name: "later service",
            issue_kind: "later-service-failed",
            start: Box::new(|| {
                later_started.set(true);
                Ok(())
            }),
        },
    ];

    run_runtime_services(services, |name, kind, error| {
        failures.borrow_mut().push((name, kind, error));
    });

    assert!(later_started.get());
    let failures = failures.into_inner();
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0].0, "returned failure");
    assert_eq!(failures[0].1, "returned-failure");
    assert_eq!(failures[0].2, "could not start");
    assert_eq!(failures[1].0, "panic failure");
    assert_eq!(failures[1].1, "panic-failure");
    assert_eq!(failures[1].2, "service panic");
}

#[test]
fn a_failure_reporter_panic_does_not_skip_later_admission() {
    let later_started = Cell::new(false);
    run_runtime_services(
        vec![
            RuntimeService {
                name: "failed service",
                issue_kind: "failed-service",
                start: Box::new(|| Err("could not start".to_string())),
            },
            RuntimeService {
                name: "later service",
                issue_kind: "later-service",
                start: Box::new(|| {
                    later_started.set(true);
                    Ok(())
                }),
            },
        ],
        |_, _, _| panic!("reporter panic"),
    );

    assert!(later_started.get());
}

#[test]
fn managed_update_attempt_guard_uses_app_wide_attempt_freshness() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-10T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert!(managed_update_attempt_eligible(None, now));
    assert!(managed_update_attempt_eligible(Some("invalid"), now));
    assert!(managed_update_attempt_eligible(Some("2026-09-10T00:00:01Z"), now));
    assert!(managed_update_attempt_eligible(Some("2026-09-09T00:00:00Z"), now));
    assert!(!managed_update_attempt_eligible(Some("2026-09-09T00:00:01Z"), now));
}

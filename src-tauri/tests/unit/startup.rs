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
fn a_stored_ignored_name_that_is_not_a_file_name_does_not_stop_the_launch() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join(crate::storage::CONFIG_FILE_NAME);
    let bytes = br#"{"formatVersion":1,"ignoredFileNames":["local.ini","photos/raw.jpg"],"theme":"dark"}"#;
    std::fs::write(&config, bytes).unwrap();

    prepare_data(temp.path()).expect("the launch continues on the built-in list");

    let held = crate::storage::config(temp.path()).unwrap();
    assert_eq!(held["ignoredFileNames"], crate::storage::effective_config(None)["ignoredFileNames"]);
    assert_eq!(held["theme"], "dark", "the other sets are kept");
    assert_eq!(std::fs::read(&config).unwrap(), bytes, "the launch writes nothing");
}

#[test]
fn blocked_gate_exposes_only_stable_private_safe_copy() {
    let failure = StartupGate::blocked().failure().unwrap();
    // The fixed sentences, with nothing from the failure interpolated.
    assert_eq!(failure.title, BLOCKED_TITLE);
    assert_eq!(failure.message, BLOCKED_MESSAGE);
    assert!(!failure.message.contains('/'));
    assert!(!failure.message.contains("sqlite"));
}

#[test]
fn required_stores_a_newer_onecopy_wrote_stop_the_launch_by_name_untouched() {
    let temp = tempfile::tempdir().unwrap();
    assert!(newer_required_stores(temp.path()).unwrap().is_empty(), "absent stores are not newer");
    let index = temp.path().join(crate::storage::INDEX_DB_FILE_NAME);
    {
        let connection = rusqlite::Connection::open(&index).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
    }
    let config = temp.path().join(crate::storage::CONFIG_FILE_NAME);
    std::fs::write(&config, br#"{"formatVersion":2}"#).unwrap();
    let index_bytes = std::fs::read(&index).unwrap();

    let newer = newer_required_stores(temp.path()).unwrap();

    assert_eq!(
        newer.iter().map(|store| store.file.as_str()).collect::<Vec<_>>(),
        [crate::storage::INDEX_DB_FILE_NAME, crate::storage::CONFIG_FILE_NAME]
    );
    assert_eq!(std::fs::read(&index).unwrap(), index_bytes);
    assert_eq!(std::fs::read(&config).unwrap(), br#"{"formatVersion":2}"#);
    assert!(!temp.path().join(crate::records::RECORDS_DB_FILE_NAME).exists());
    let failure = StartupGate::newer(newer).failure().unwrap();
    assert_eq!(failure.newer_stores.len(), 2);
    assert!(StartupGate::blocked().failure().unwrap().newer_stores.is_empty());
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

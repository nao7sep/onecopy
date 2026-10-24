use super::*;
use std::cell::Cell;

fn labels(names: &[&str]) -> HashSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}
fn release(names: &[&str]) -> Release {
    Release {
        pending: labels(names),
        keys: vec!["hash".into()],
        restore_playback: false,
    }
}

#[test]
fn every_live_window_must_acknowledge_before_admission() {
    let registry = ReleaseRegistry::default();
    let resumed = Cell::new(false);
    let result = registry.acquire(
        1,
        release(&["main", "preview"]),
        Instant::now(),
        || {
            assert!(registry.acknowledge(1, "main").unwrap());
            assert!(!registry.acknowledge(999, "preview").unwrap());
            Ok(())
        },
        || labels(&["main", "preview"]),
        || {
            assert!(registry.current("late").unwrap().is_none());
            assert!(!registry.acknowledge(1, "preview").unwrap());
            resumed.set(true);
        },
    );
    assert!(result.err().unwrap().contains("preview"));
    assert!(resumed.get());

    let lease = registry
        .acquire(
            2,
            release(&["main", "preview"]),
            Instant::now(),
            || {
                assert!(registry.acknowledge(2, "main").unwrap());
                assert!(registry.acknowledge(2, "preview").unwrap());
                Ok(())
            },
            || labels(&["main", "preview"]),
            || panic!("admitted release must not resume early"),
        )
        .unwrap();
    assert!(registry.acknowledge(2, "main").unwrap());
    drop(lease);
    assert!(!registry.acknowledge(2, "main").unwrap());
}

#[test]
fn bootstrap_joins_pending_admission_and_retired_tokens_do_not_survive_timeout() {
    let registry = ReleaseRegistry::default();
    let resumed = Cell::new(0);
    let result = registry.acquire(
        1,
        release(&["main"]),
        Instant::now(),
        || {
            assert_eq!(
                registry.current("preview").unwrap(),
                Some(json!({
                    "token": 1, "keys": ["hash"], "restorePlayback": false,
                }))
            );
            registry.acknowledge(1, "main")?;
            Ok(())
        },
        || labels(&["main", "preview"]),
        || resumed.set(resumed.get() + 1),
    );
    assert!(result.err().unwrap().contains("preview"));
    assert_eq!(resumed.get(), 1);
    assert!(registry.current("next").unwrap().is_none());
}

#[test]
fn closed_windows_are_removed_from_the_pending_set() {
    let registry = ReleaseRegistry::default();
    let lease = registry
        .acquire(
            1,
            release(&["main", "closed"]),
            Instant::now(),
            || {
                registry.acknowledge(1, "main")?;
                Ok(())
            },
            || labels(&["main"]),
            || panic!("closed windows cannot hold admission"),
        )
        .unwrap();
    drop(lease);
    assert!(registry.current("next").unwrap().is_none());
}

#[test]
fn an_empty_initial_window_set_still_blocks_late_bootstrap_for_the_guard_lifetime() {
    let registry = ReleaseRegistry::default();
    let lease = registry
        .acquire(
            1,
            release(&[]),
            Instant::now(),
            || Ok(()),
            HashSet::new,
            || {},
        )
        .unwrap();
    assert_eq!(
        registry.current("new-window").unwrap(),
        Some(json!({
            "token": 1, "keys": ["hash"], "restorePlayback": false,
        }))
    );
    assert!(registry.acknowledge(1, "new-window").unwrap());
    drop(lease);
    assert!(registry.current("another-window").unwrap().is_none());
    assert!(!registry.acknowledge(1, "new-window").unwrap());
}

#[test]
fn failed_publication_retires_admission_before_resuming_partially_notified_windows() {
    let registry = ReleaseRegistry::default();
    let resumed = Cell::new(0);
    let result = registry.acquire(
        1,
        release(&["main"]),
        Instant::now(),
        || Err("emit failed".into()),
        || panic!("failed publication must not wait"),
        || {
            assert!(registry.current("main").unwrap().is_none());
            resumed.set(resumed.get() + 1);
        },
    );
    assert_eq!(result.err().unwrap(), "emit failed");
    assert_eq!(resumed.get(), 1);
}

#[test]
fn acknowledgement_wakes_the_actual_pending_wait() {
    let registry = ReleaseRegistry::default();
    std::thread::scope(|scope| {
        let (observed, observation) = std::sync::mpsc::sync_channel(0);
        let first = std::sync::atomic::AtomicBool::new(true);
        let registry = &registry;
        let worker = scope.spawn(move || {
            registry.acquire(
                1,
                release(&["main"]),
                Instant::now() + Duration::from_secs(5),
                || Ok(()),
                || {
                    if first.swap(false, Ordering::Relaxed) {
                        observed.send(()).unwrap();
                    }
                    labels(&["main"])
                },
                || panic!("acknowledged admission must not time out"),
            )
        });
        observation.recv().unwrap();
        registry.acknowledge(1, "main").unwrap();
        let lease = worker.join().unwrap().unwrap();
        assert!(registry.acknowledge(1, "main").unwrap());
        drop(lease);
    });
    assert!(registry.current("main").unwrap().is_none());
}

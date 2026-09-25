use super::*;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

fn holder_present() -> bool {
    index_state().holder.is_some()
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition never became true");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn cancellation_is_revalidated_after_the_projection_lock_is_owned() {
    let _serial = serial_test();
    let ran = std::cell::Cell::new(false);

    let result = with_owner(
        Owner::Watcher,
        || true,
        || {
            ran.set(true);
            Ok(())
        },
    );

    assert_eq!(result.unwrap_err(), crate::scanner::CANCELLED);
    assert!(!ran.get());
    assert_eq!(ACTIVE_OWNER.load(Ordering::SeqCst), 0);
    assert!(!crate::scanner::SCAN_CANCEL.load(Ordering::SeqCst));
}

#[test]
fn a_foreground_wait_shows_itself_answers_busy_at_its_deadline_and_ends_on_cancel() {
    let _serial = serial_test();
    // Another foreground action holds the claim and cannot be preempted.
    let first = admit_foreground(None, None, &|| false, &mut || {}).unwrap();

    let deadline = Instant::now() + Duration::from_millis(150);
    let busy = admit_foreground(None, Some(deadline), &|| false, &mut || {});
    assert!(matches!(busy, Err(Refusal::Busy)));
    assert!(Instant::now() < deadline + Duration::from_millis(500));

    let cancel = AtomicBool::new(false);
    let shown = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| {
            admit_foreground(None, None, &|| cancel.load(Ordering::SeqCst), &mut || {
                shown.store(true, Ordering::SeqCst)
            })
            .map(|_| ())
        });
        wait_until(|| shown.load(Ordering::SeqCst));
        let requested = Instant::now();
        cancel.store(true, Ordering::SeqCst);
        assert!(matches!(waiter.join().unwrap(), Err(Refusal::Cancelled)));
        assert!(requested.elapsed() < Duration::from_millis(500));
    });
    drop(first);
    assert!(!holder_present());
    assert!(!foreground_pending());
}

#[test]
fn a_parked_source_walk_continues_after_foreground_work_and_restarts_after_a_reset() {
    let _serial = serial_test();
    for reset in [false, true] {
        let parked = Arc::new(AtomicBool::new(false));
        let observed = parked.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let walker = std::thread::spawn(move || {
            with_source_check_claim(
                || false,
                move |waiting| observed.store(waiting, Ordering::SeqCst),
                || {
                    entered_tx.send(()).unwrap();
                    let mut steps = 0u32;
                    while steps < 100 {
                        yield_to_foreground()?;
                        steps += 1;
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Ok(steps)
                },
            )
        });
        entered_rx.recv().unwrap();

        let requested = Instant::now();
        let foreground = admit_foreground(
            None,
            Some(requested + Duration::from_secs(5)),
            &|| false,
            &mut || {},
        )
        .unwrap();
        assert!(requested.elapsed() < Duration::from_secs(1));
        assert!(parked.load(Ordering::SeqCst));
        assert_eq!(ACTIVE_OWNER.load(Ordering::SeqCst), Owner::Foreground as u8);
        if reset {
            restart_source_walks();
        }
        drop(foreground);

        let result = walker.join().unwrap();
        if reset {
            assert_eq!(result.unwrap_err(), RESTART);
        } else {
            // The walk kept its progress instead of starting over.
            assert_eq!(result.unwrap(), 100);
        }
        assert!(!parked.load(Ordering::SeqCst));
        assert_eq!(ACTIVE_OWNER.load(Ordering::SeqCst), 0);
        assert!(!holder_present());
    }
}

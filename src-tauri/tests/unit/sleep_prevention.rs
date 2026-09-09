use super::*;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, ThreadId};
use std::time::Duration;

#[derive(Debug, PartialEq)]
enum Event {
    Acquired(ThreadId),
    Released(ThreadId),
    Failed,
    Recovered,
}

struct Assertion {
    events: Sender<Event>,
    owner: ThreadId,
    // The production lease need not be Send: neither should this fixture.
    _thread_affine: std::rc::Rc<()>,
}

impl Drop for Assertion {
    fn drop(&mut self) {
        assert_eq!(self.owner, thread::current().id());
        self.events.send(Event::Released(self.owner)).unwrap();
    }
}

struct Harness {
    control: Arc<Control>,
    events: Receiver<Event>,
    worker: Option<JoinHandle<Result<(), String>>>,
}

impl Harness {
    fn new(enabled: bool, fail_first: bool) -> Self {
        let control = Arc::new(Control::default());
        control.configure(enabled);
        let target = control.clone();
        let (send, events) = channel();
        let failures = send.clone();
        let recovered = send.clone();
        let worker = thread::spawn(move || {
            let mut fail = fail_first;
            let mut first_recovery = true;
            run(
                &target,
                move || {
                    if std::mem::take(&mut fail) {
                        return Err("injected OS rejection".to_string());
                    }
                    let owner = thread::current().id();
                    send.send(Event::Acquired(owner)).unwrap();
                    Ok(Assertion {
                        events: send.clone(),
                        owner,
                        _thread_affine: std::rc::Rc::new(()),
                    })
                },
                |_| {
                    failures.send(Event::Failed).unwrap();
                    Ok(())
                },
                || {
                    if !std::mem::take(&mut first_recovery) || fail_first {
                        recovered.send(Event::Recovered).unwrap();
                    }
                    Ok(())
                },
            )
        });
        Self {
            control,
            events,
            worker: Some(worker),
        }
    }

    fn next(&self) -> Event {
        self.events
            .recv_timeout(Duration::from_secs(3))
            .expect("owner did not settle")
    }

    fn acquired(&self) -> ThreadId {
        match self.next() {
            Event::Acquired(owner) => owner,
            event => panic!("expected acquisition, got {event:?}"),
        }
    }

    fn finish(&mut self) {
        self.control.update(|state| state.stopped = true);
        self.worker.take().unwrap().join().unwrap().unwrap();
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.control.update(|state| state.stopped = true);
            worker.join().unwrap().unwrap();
        }
    }
}

#[test]
fn policy_requires_enabled_actual_work_and_a_live_application() {
    for enabled in [false, true] {
        for active in [0, 1, 3] {
            for stopped in [false, true] {
                let state = State {
                    enabled,
                    active,
                    stopped,
                    ..State::default()
                };
                assert_eq!(state.wanted(), enabled && active > 0 && !stopped);
            }
        }
    }
    assert!(configured(None));
    assert!(!configured(Some(
        &serde_json::json!({"keepAwakeDuringIndexing": false})
    )));
}

#[test]
fn failed_assertion_is_inert_until_the_preference_changes() {
    let control = Arc::new(Control::default());
    control.configure(true);
    let first = control.begin();
    let failed_preference = control.state.lock().unwrap().preference_revision;
    drop(first);
    let later = control.begin();
    control.configure(true);
    let state = *control.state.lock().unwrap();
    assert!(state.wanted());
    assert!(!state.should_acquire(false, Some(failed_preference)));
    assert!(!state.should_acquire(true, None));
    control.configure(false);
    control.configure(true);
    assert!(control
        .state
        .lock()
        .unwrap()
        .should_acquire(false, Some(failed_preference)));
    drop(later);
}

#[test]
fn overlapping_work_shares_assertion_and_new_discoveries_reacquire_on_the_same_thread() {
    let mut harness = Harness::new(true, false);
    for _ in 0..3 {
        let source = harness.control.begin();
        let owner = harness.acquired();
        assert_ne!(owner, thread::current().id());
        let information = harness.control.begin();
        let preview = harness.control.begin();
        drop(source);
        drop(information);
        assert!(harness.control.state.lock().unwrap().wanted());
        drop(preview);
        assert_eq!(harness.next(), Event::Released(owner));
    }
    harness.finish();
    assert!(harness.events.try_recv().is_err());
}

#[test]
fn live_preference_disables_and_reenables_already_running_work() {
    let mut harness = Harness::new(false, false);
    let work = harness.control.begin();
    assert!(!harness.control.state.lock().unwrap().wanted());
    harness.control.configure(true);
    let owner = harness.acquired();
    harness.control.configure(false);
    assert_eq!(harness.next(), Event::Released(owner));
    harness.control.configure(true);
    assert_eq!(harness.acquired(), owner);
    drop(work);
    assert_eq!(harness.next(), Event::Released(owner));
    harness.finish();
}

#[test]
fn failure_does_not_retry_on_queue_churn_but_preference_toggle_recovers() {
    let mut harness = Harness::new(true, true);
    let work = harness.control.begin();
    assert_eq!(harness.next(), Event::Failed);
    // New jobs, including new busy periods, do not hammer a failing OS call.
    drop(work);
    let later = harness.control.begin();
    harness.control.configure(true);
    harness.control.configure(false);
    harness.control.configure(true);
    let owner = harness.acquired();
    assert_eq!(harness.next(), Event::Recovered);
    drop(later);
    assert_eq!(harness.next(), Event::Released(owner));
    harness.finish();
}

#[test]
fn shutdown_releases_even_when_workers_still_hold_guards() {
    let mut harness = Harness::new(true, false);
    let work = harness.control.begin();
    let owner = harness.acquired();
    harness.finish();
    assert_eq!(harness.next(), Event::Released(owner));
    let late = harness.control.begin();
    assert!(!harness.control.state.lock().unwrap().wanted());
    drop((work, late));
    assert!(harness.events.try_recv().is_err());
}

#[test]
fn errors_cancellation_and_panics_release_the_work_scope() {
    let mut harness = Harness::new(true, false);
    for outcome in ["error", "cancel", "panic"] {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _work = harness.control.begin();
            let owner = harness.acquired();
            match outcome {
                "panic" => panic!("injected worker panic"),
                "cancel" => Err::<(), _>((owner, crate::scanner::CANCELLED)),
                _ => Err((owner, "injected work failure")),
            }
        }));
        assert_eq!(harness.control.state.lock().unwrap().active, 0);
        assert!(matches!(harness.next(), Event::Released(_)));
        if outcome == "panic" {
            assert!(result.is_err());
        }
    }
    harness.finish();
}

#[test]
fn disable_during_native_acquisition_cannot_lose_the_release_wakeup() {
    let control = Arc::new(Control::default());
    control.configure(true);
    let work = control.begin();
    let target = control.clone();
    let (entered, ready) = channel();
    let (proceed, resume) = channel();
    let (events, output) = channel();
    let worker = thread::spawn(move || {
        run(
            &target,
            || {
                entered.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(3)).unwrap();
                Ok(Assertion {
                    events: events.clone(),
                    owner: thread::current().id(),
                    _thread_affine: std::rc::Rc::new(()),
                })
            },
            |_| Ok(()),
            || Ok(()),
        )
    });
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    control.configure(false);
    proceed.send(()).unwrap();
    assert!(matches!(
        output.recv_timeout(Duration::from_secs(3)).unwrap(),
        Event::Released(_)
    ));
    control.update(|state| state.stopped = true);
    worker.join().unwrap().unwrap();
    drop(work);
}

#[test]
fn reporting_failure_terminates_only_the_assertion_owner() {
    let control = Arc::new(Control::default());
    control.configure(true);
    let work = control.begin();
    let result = run::<()>(
        &control,
        || Err("OS rejected".to_string()),
        |_| Err("diagnostic store unavailable".to_string()),
        || Ok(()),
    );
    assert_eq!(result, Err("diagnostic store unavailable".to_string()));
    assert_eq!(control.state.lock().unwrap().active, 1);
    drop(work);
    assert_eq!(control.state.lock().unwrap().active, 0);
}

#[test]
fn first_success_resolves_a_condition_retained_from_a_previous_run() {
    let control = Arc::new(Control::default());
    control.configure(true);
    let work = control.begin();
    let mut recovered = false;
    run(
        &control,
        || Ok(()),
        |_| panic!("unexpected failure"),
        || {
            recovered = true;
            control.update(|state| state.stopped = true);
            Ok(())
        },
    )
    .unwrap();
    assert!(recovered);
    drop(work);
}

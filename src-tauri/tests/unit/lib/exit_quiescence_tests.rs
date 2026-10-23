use super::{await_other_joins_with_deadline, run_exit, spawn_thread, ExitSequence};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// The real subprocess kill would also end subprocesses that parallel tests
// own, so these sequences count calls instead.
fn no_kill() {}

#[test]
fn a_signal_before_the_deadline_finishes_without_killing_anything() {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    tx.send(true).unwrap();
    assert!(await_other_joins_with_deadline(rx, Duration::from_secs(5), || {
        panic!("nothing is killed before the deadline")
    }));
}

// Quitting must not wait past its deadline for a job that never reaches its
// own cancellation point.
#[test]
fn no_signal_by_the_deadline_gives_up_waiting() {
    static KILLS: AtomicUsize = AtomicUsize::new(0);
    let (_tx, rx) = std::sync::mpsc::channel::<bool>();
    let started = Instant::now();
    assert!(!await_other_joins_with_deadline(rx, Duration::from_millis(50), || {
        KILLS.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(KILLS.load(Ordering::SeqCst), 1, "outstanding subprocesses are ended at the deadline");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "must give up at the deadline, not hang"
    );
}

type Steps = Arc<Mutex<Vec<String>>>;

fn sequence(steps: &Steps, join_workers: Box<dyn FnOnce() + Send>, mutation: Duration) -> (ExitSequence, std::sync::mpsc::Receiver<()>) {
    let (exited_tx, exited_rx) = std::sync::mpsc::channel();
    let mutation_steps = steps.clone();
    let exit_steps = steps.clone();
    let report_steps = steps.clone();
    (
        ExitSequence {
            join_workers: Box::new(move || { join_workers(); true }),
            wait_for_mutation: Box::new(move || {
                std::thread::sleep(mutation);
                mutation_steps.lock().unwrap().push("mutation idle".into());
                Ok(crate::mutation_runtime::IdleWait::Idle)
            }),
            exit: Box::new(move |_| {
                exit_steps.lock().unwrap().push("exit".into());
                let _ = exited_tx.send(());
            }),
            report: Arc::new(move |error| {
                report_steps.lock().unwrap().push(format!("reported: {error}"));
            }),
            kill_subprocesses: no_kill,
        },
        exited_rx,
    )
}

// A mutation that reaches its own safe point well inside its deadline is
// never treated as timed out, even while a stuck derived join is abandoned at
// its own, shorter deadline.
#[test]
fn a_mutation_that_reaches_its_safe_point_in_time_exits_normally() {
    let steps = Steps::default();
    let (_never, stuck) = std::sync::mpsc::channel::<()>();
    let (exit, exited) = sequence(
        &steps,
        Box::new(move || {
            let _ = stuck.recv();
        }),
        Duration::from_millis(300),
    );
    let started = Instant::now();
    run_exit(exit, spawn_thread, Duration::from_millis(50));
    exited.recv_timeout(Duration::from_secs(30)).expect("exit is reached");

    assert!(started.elapsed() >= Duration::from_millis(300));
    assert_eq!(*steps.lock().unwrap(), vec!["mutation idle", "exit"]);
}

// A file operation that never reaches its own safe point lets exit complete
// once the mutation-quiescence deadline passes: quitting gives up on it
// rather than waiting forever, per the same "Normal exit and abnormal
// termination" contract.
#[test]
fn a_mutation_that_never_reaches_a_safe_point_still_lets_exit_complete() {
    let steps = Steps::default();
    let mutation_steps = steps.clone();
    let exit_steps = steps.clone();
    let (exited_tx, exited_rx) = std::sync::mpsc::channel();
    let sequence = ExitSequence {
        join_workers: Box::new(|| true),
        wait_for_mutation: Box::new(move || {
            mutation_steps.lock().unwrap().push("mutation timed out".into());
            Ok(crate::mutation_runtime::IdleWait::TimedOut)
        }),
        exit: Box::new(move |_| {
            exit_steps.lock().unwrap().push("exit".into());
            let _ = exited_tx.send(());
        }),
        report: Arc::new(|_| {}),
        kill_subprocesses: no_kill,
    };
    let started = Instant::now();
    run_exit(sequence, spawn_thread, Duration::from_secs(5));
    exited_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("giving up on the mutation still reaches exit");

    assert!(
        started.elapsed() < Duration::from_secs(1),
        "giving up must not add its own extra wait on top of the mutation's own deadline"
    );
    assert_eq!(*steps.lock().unwrap(), vec!["mutation timed out", "exit"]);
}

fn no_exit_thread(name: &'static str, work: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    if name == "onecopy-exit" {
        return Err(std::io::Error::other("thread limit reached"));
    }
    spawn_thread(name, work)
}

// When the exit thread cannot start, the failure is reported and the
// sequence still runs to exit on the caller's thread, after mutation
// quiescence, instead of leaving the app open with its workers stopped.
#[test]
fn a_failed_exit_thread_start_still_quits_after_mutation_quiescence() {
    let steps = Steps::default();
    let (exit, exited) = sequence(&steps, Box::new(|| {}), Duration::ZERO);

    run_exit(exit, no_exit_thread, Duration::from_secs(5));

    exited.try_recv().expect("the caller's thread ran the sequence to exit");
    let steps = steps.lock().unwrap();
    assert_eq!(steps.len(), 3, "{steps:?}");
    // The report comes first and carries the spawn error itself.
    assert!(steps[0].starts_with("reported: ") && steps[0].contains("thread limit reached"), "{steps:?}");
    assert_eq!(steps[1..], ["mutation idle", "exit"]);
}


#[test]
fn unsuccessful_worker_quiescence_does_not_count_as_a_clean_exit() {
    let (tx, rx) = std::sync::mpsc::channel();
    let (clean_tx, clean_rx) = std::sync::mpsc::channel();
    let sequence = ExitSequence {
        join_workers: Box::new(|| false),
        wait_for_mutation: Box::new(|| Ok(crate::mutation_runtime::IdleWait::Idle)),
        exit: Box::new(move |clean| { clean_tx.send(clean).unwrap(); tx.send(()).unwrap(); }),
        report: Arc::new(|_| {}),
        kill_subprocesses: no_kill,
    };
    run_exit(sequence, spawn_thread, Duration::from_secs(1));
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!clean_rx.recv().unwrap());
}

#[test]
#[ignore = "disposable process fixture: invoked by the production watchdog test"]
fn stalled_exit_watchdog_fixture() {
    use std::io::Read;
    let root = std::path::PathBuf::from(std::env::var_os("ONECOPY_EXIT_WATCHDOG_FIXTURE").expect("fixture directory"));
    let takeover = std::env::var("ONECOPY_EXIT_WATCHDOG_TAKEOVER").as_deref() == Ok("1");
    super::start_exit_deadline(if takeover { super::EXIT_TOTAL_DEADLINE } else { Duration::from_secs(2) });
    let mut sequence = sequence(&Steps::default(), Box::new(|| {}), Duration::ZERO).0;
    sequence.exit = Box::new(move |_| {
        std::fs::write(root.join("cleanup-entered"), b"ready").unwrap();
        // Never release final cleanup. Only the production watchdog may exit.
        loop { std::thread::park(); }
    });
    run_exit(sequence, spawn_thread, Duration::from_secs(1));
    if takeover {
        let mut request = [0];
        std::io::stdin().read_exact(&mut request).unwrap();
        super::start_exit_deadline(super::SESSION_EXIT_DEADLINE);
        // A repeated ordinary quit cannot extend the shortened deadline.
        super::start_exit_deadline(super::EXIT_TOTAL_DEADLINE);
    }
    loop { std::thread::park(); }
}

#[test]
fn production_watchdog_exits_stalled_cleanup_and_os_takeover_shortens_it() {
    use std::io::Write;
    use std::process::{Child, Command, Stdio};
    struct Fixture(Child);
    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    for takeover in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut child = Fixture(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "app_lifecycle::exit_quiescence_tests::stalled_exit_watchdog_fixture", "--ignored", "--nocapture"])
            .env("ONECOPY_EXIT_WATCHDOG_FIXTURE", root.path())
            .env("ONECOPY_EXIT_WATCHDOG_TAKEOVER", if takeover { "1" } else { "0" })
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().unwrap());
        let ready_deadline = Instant::now() + Duration::from_secs(10);
        while !root.path().join("cleanup-entered").is_file() {
            assert!(child.0.try_wait().unwrap().is_none(), "fixture exited before entering final cleanup");
            assert!(Instant::now() < ready_deadline, "fixture never entered final cleanup");
            std::thread::sleep(Duration::from_millis(10));
        }
        if takeover { child.0.stdin.take().unwrap().write_all(b"1").unwrap(); }
        let exit_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "the production watchdog must perform its normal forced exit");
                break;
            }
            assert!(Instant::now() < exit_deadline, "the real watchdog did not terminate stalled cleanup");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn os_takeover_shortens_a_running_exit_budget_and_repeats_never_extend_it() {
    let budget = Arc::new(super::ExitBudget::new());
    budget.arm(Duration::from_secs(30));
    let worker_budget = budget.clone();
    let (expired, observed) = std::sync::mpsc::channel();
    let watchdog = std::thread::spawn(move || { worker_budget.wait(); expired.send(()).unwrap(); });
    assert!(!budget.arm(Duration::from_millis(30)));
    assert!(!budget.arm(Duration::from_secs(60)));
    observed.recv_timeout(Duration::from_secs(2)).expect("OS takeover wakes the existing owner and cannot extend it");
    watchdog.join().unwrap();
}

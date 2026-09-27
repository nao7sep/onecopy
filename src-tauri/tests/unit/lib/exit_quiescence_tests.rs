use super::{await_other_joins_with_deadline, run_exit, spawn_thread, ExitSequence};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
fn a_signal_before_the_deadline_finishes_without_killing_anything() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    tx.send(()).unwrap();
    assert!(await_other_joins_with_deadline(rx, Duration::from_secs(5)));
}

// Quitting must not wait past its deadline for a job that never reaches its
// own cancellation point.
#[test]
fn no_signal_by_the_deadline_gives_up_waiting() {
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    let started = Instant::now();
    assert!(!await_other_joins_with_deadline(rx, Duration::from_millis(50)));
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
            join_workers,
            wait_for_mutation: Box::new(move || {
                std::thread::sleep(mutation);
                mutation_steps.lock().unwrap().push("mutation idle".into());
                Ok(crate::mutation_runtime::IdleWait::Idle)
            }),
            exit: Box::new(move || {
                exit_steps.lock().unwrap().push("exit".into());
                let _ = exited_tx.send(());
            }),
            report: Arc::new(move |error| {
                report_steps.lock().unwrap().push(format!("reported: {error}"));
            }),
        },
        exited_rx,
    )
}

// A mutation that reaches its own safe point well inside its deadline is
// never treated as timed out, even while a stuck derived join is abandoned at
// its own, shorter deadline (`specs/file-operations.md`, "Normal exit and
// abnormal termination").
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
        join_workers: Box::new(|| {}),
        wait_for_mutation: Box::new(move || {
            mutation_steps.lock().unwrap().push("mutation timed out".into());
            Ok(crate::mutation_runtime::IdleWait::TimedOut)
        }),
        exit: Box::new(move || {
            exit_steps.lock().unwrap().push("exit".into());
            let _ = exited_tx.send(());
        }),
        report: Arc::new(|_| {}),
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

// (R4.4 B) When the exit thread cannot start, the failure is reported and the
// sequence still runs to exit on the caller's thread, after mutation
// quiescence, instead of leaving the app open with its workers stopped.
#[test]
fn a_failed_exit_thread_start_still_quits_after_mutation_quiescence() {
    let steps = Steps::default();
    let (exit, exited) = sequence(&steps, Box::new(|| {}), Duration::ZERO);

    run_exit(exit, no_exit_thread, Duration::from_secs(5));

    exited.try_recv().expect("the caller's thread ran the sequence to exit");
    assert_eq!(
        *steps.lock().unwrap(),
        vec![
            "reported: could not start the exit thread: thread limit reached",
            "mutation idle",
            "exit",
        ]
    );
}

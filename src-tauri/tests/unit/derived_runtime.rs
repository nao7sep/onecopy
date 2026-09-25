use super::*;
use std::sync::mpsc;
use std::thread;

fn job(class: WorkClass, manual: bool) -> Job {
    Job {
        id: 1,
        class,
        manual,
        hash: Some("hash".to_string()),
        stop: false,
        done: None,
        total: None,
    }
}

fn set_hash(id: u64, hash: &str) {
    let mut runtime = RUNTIME.0.lock().unwrap();
    runtime
        .jobs_mut()
        .find(|job| job.id == id)
        .unwrap()
        .hash = Some(hash.to_string());
}

/// Runs an automatic heavy job for `hash` on its own thread until its safe
/// point reports a stop, like a long transcription polling `cancelled`.
fn long_automatic_job(
    class: WorkClass,
    hash: &'static str,
) -> (thread::JoinHandle<Duration>, mpsc::Receiver<()>) {
    let (running_tx, running_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let job = ActiveGuard::begin(None, class).unwrap().unwrap();
        set_hash(job.id, hash);
        running_tx.send(()).unwrap();
        let started = Instant::now();
        while !cancelled() {
            assert!(started.elapsed() < Duration::from_secs(20), "the job was never stopped");
            thread::sleep(Duration::from_millis(5));
        }
        started.elapsed()
    });
    (worker, running_rx)
}

#[test]
fn shutdown_releases_every_manual_ticket_from_its_queue_wait() {
    let runtime = RuntimeState {
        claim: Some(Claim { keys: Vec::new() }),
        heavy: Some(job(WorkClass::VideoTranscripts, true)),
        serving_manual_ticket: 2,
        ..RuntimeState::default()
    };
    assert!(manual_waits(&runtime, 7, Some("other"), false));
    assert!(!manual_waits(&runtime, 7, Some("other"), true));
}

#[test]
fn foreground_admission_stops_automatic_work_and_never_requested_work() {
    let runtime = RuntimeState::default();
    let automatic = job(WorkClass::VideoTranscripts, false);
    let requested = job(WorkClass::VideoTranscripts, true);
    assert!(!job_cancelled(&runtime, &automatic, false, false));
    assert!(job_cancelled(&runtime, &automatic, false, true));
    assert!(!job_cancelled(&runtime, &requested, false, true));
    assert!(job_cancelled(&runtime, &requested, true, false));
}

#[test]
fn a_file_operation_starts_within_a_long_automatic_jobs_safe_point() {
    let _serial = crate::scan_runtime::serial_test();
    let (held_tx, held_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        crate::scan_runtime::try_with_derived_claim(|| {
            let _job = ActiveGuard::begin(None, WorkClass::VideoTranscripts)
                .unwrap()
                .unwrap();
            held_tx.send(()).unwrap();
            // A long fake derived job whose only safe point is `cancelled`.
            let started = Instant::now();
            while !cancelled() {
                assert!(started.elapsed() < Duration::from_secs(20), "the job never yielded");
                thread::sleep(Duration::from_millis(5));
            }
        })
    });
    held_rx.recv().unwrap();

    let requested = Instant::now();
    let admitted = crate::scan_runtime::admit_foreground(
        None,
        Some(requested + Duration::from_secs(5)),
        &|| false,
        &mut || {},
    );
    assert!(admitted.is_ok(), "{:?}", admitted.err());
    assert!(requested.elapsed() < Duration::from_secs(2));
    drop(admitted);
    assert!(worker.join().unwrap().is_some());
}

#[test]
fn an_external_open_never_stops_requested_work_and_stops_automatic_work_only_on_its_file() {
    let _serial = crate::scan_runtime::serial_test();
    let requested = requested_heavy(None, WorkClass::VideoTranscripts, "movie").unwrap();
    let opened = exclusive_claim(None, &["movie".to_string()], ClaimPolicy::ExternalOpen, &|| false);
    assert!(opened.is_ok());
    assert!(!cancelled(), "opening the file externally stopped the requested transcription");
    drop(opened);
    drop(requested);

    let (worker, running) = long_automatic_job(WorkClass::Faces, "photo");
    running.recv().unwrap();
    let elsewhere = Instant::now();
    drop(exclusive_claim(None, &["other".to_string()], ClaimPolicy::ExternalOpen, &|| false).unwrap());
    assert!(elsewhere.elapsed() < Duration::from_secs(1));
    let same = exclusive_claim(None, &["photo".to_string()], ClaimPolicy::ExternalOpen, &|| false);
    assert!(same.is_ok());
    drop(same);
    worker.join().unwrap();
}

#[test]
fn a_file_operation_stops_requested_work_on_its_own_file() {
    let _serial = crate::scan_runtime::serial_test();
    let (running_tx, running_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _job = requested_heavy(None, WorkClass::AudioTranscripts, "song").unwrap();
        running_tx.send(()).unwrap();
        let started = Instant::now();
        while !cancelled() {
            assert!(started.elapsed() < Duration::from_secs(20), "the job was never stopped");
            thread::sleep(Duration::from_millis(5));
        }
    });
    running_rx.recv().unwrap();
    let claimed = exclusive_claim(None, &["song".to_string()], ClaimPolicy::Mutation, &|| false);
    assert!(claimed.is_ok());
    drop(claimed);
    worker.join().unwrap();
}

#[test]
fn a_requested_preview_runs_beside_a_transcription() {
    let _serial = crate::scan_runtime::serial_test();
    let transcription = requested_heavy(None, WorkClass::VideoTranscripts, "movie").unwrap();
    let requested = Instant::now();
    let preview = requested_preview(None, "photo");
    assert!(preview.is_ok(), "{:?}", preview.err());
    assert!(requested.elapsed() < Duration::from_secs(1));
    assert!(!cancelled());
    drop(preview);
    // Automatic preview preparation is not stopped by it either.
    let automatic = ActiveGuard::begin(None, WorkClass::Previews).unwrap();
    assert!(automatic.is_some());
    drop(automatic);
    drop(transcription);
}

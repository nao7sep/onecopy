use super::*;

#[cfg(unix)]
struct SupervisedChild(Arc<Mutex<OwnedProcess>>);

#[cfg(unix)]
impl SupervisedChild {
    fn spawn(mut command: Command) -> Self {
        let guardian = crate::subprocess_supervisor::prepare(&mut command).unwrap();
        Self(Arc::new(Mutex::new(OwnedProcess::new(command.spawn().unwrap(), guardian))))
    }
}

#[cfg(unix)]
impl Drop for SupervisedChild {
    fn drop(&mut self) {
        let mut child = lock_child(&self.0);
        kill_owned(&mut child);
        // These controlled fixtures perform no volume I/O. Reap them even if
        // an assertion fails; the guardian owns descendant termination.
        let _ = child.wait();
    }
}

#[test]
fn a_contended_registry_cannot_block_the_force_exit_signal() {
    static REGISTRY: Mutex<Vec<Arc<Mutex<OwnedProcess>>>> = Mutex::new(Vec::new());
    let held = REGISTRY.lock().unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || { signal_for_exit(&REGISTRY); done.send(()).unwrap(); });
    let returned = finished.recv_timeout(Duration::from_secs(2)).is_ok();
    drop(held);
    worker.join().unwrap();
    assert!(returned, "a held registry must not extend forced exit");
}

#[cfg(unix)]
#[test]
fn a_deadline_signal_ends_the_live_group_and_leaves_reaping_to_its_owner() {
    let mut command = Command::new("sleep");
    command.arg("60");
    let owner = SupervisedChild::spawn(command);
    let registry = Mutex::new(vec![owner.0.clone()]);
    signal_for_exit(&registry);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if lock_child(&owner.0).try_wait().unwrap().is_some() { break; }
        assert!(Instant::now() < deadline, "deadline signal did not end the child");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Whether `pid` names a process that has not exited.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only probes whether the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: a null handle is checked before use and the handle is closed.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        let read = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        read != 0 && exit_code == STILL_ACTIVE as u32
    }
}

// Windows has no process group to signal: the deadline signal ends the job
// that holds the tool, and with it every process the tool started.
windows_test! {
    "Windows only: job objects",
    fn a_deadline_signal_ends_the_whole_job_and_leaves_reaping_to_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-Command",
            &format!(
                "$p = Start-Process -FilePath ping -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; \
                 Set-Content -LiteralPath '{}' -Value $p.Id; Start-Sleep -Seconds 60",
                pid_file.display()
            ),
        ]);
        // Dropping the owner closes the job, which ends anything a failed
        // assertion leaves running.
        let owner = Arc::new(Mutex::new(OwnedProcess::new(command.spawn().unwrap())));
        let started = Instant::now();
        let grandchild = loop {
            let recorded = std::fs::read_to_string(&pid_file).ok().and_then(|text| text.trim().parse::<u32>().ok());
            if let Some(pid) = recorded { break pid; }
            assert!(started.elapsed() < Duration::from_secs(30), "the tool never recorded the process it started");
            std::thread::sleep(Duration::from_millis(20));
        };
        let registry = Mutex::new(vec![owner.clone()]);
        signal_for_exit(&registry);
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(grandchild) || lock_child(&owner).try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the deadline signal left part of the tool's tree running");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
#[serial_test::serial(subprocess)]
fn cancelling_a_tool_ends_what_it_started() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    // The tool starts a long-running process of its own, records its id and
    // waits; the cancel arrives once the id is recorded.
    #[cfg(unix)]
    let command = {
        let mut command = Command::new("sh");
        command.arg("-c").arg(format!("sleep 60 & echo $! > '{}'; wait", pid_file.display()));
        command
    };
    #[cfg(windows)]
    let command = {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-Command",
            &format!(
                "$p = Start-Process -FilePath ping -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; \
                 Set-Content -LiteralPath '{}' -Value $p.Id; Start-Sleep -Seconds 60",
                pid_file.display()
            ),
        ]);
        command
    };
    let recorded = || {
        std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
    };
    let started = Instant::now();
    let result = run_bounded(command, &|| recorded().is_some() || started.elapsed() > Duration::from_secs(30));
    assert!(result.is_err(), "a cancelled tool reports cancellation");
    let grandchild = recorded().expect("the tool recorded the process it started");
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(grandchild) {
        assert!(Instant::now() < deadline, "the process the tool started outlived the cancel");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_error_carries_the_whole_of_stderr() {
    let stderr = format!("first line\n{}\nlast line", "x".repeat(10_000));
    let run = Run { status_ok: false, stdout: Vec::new(), stderr: format!("  {stderr}\n") };
    assert_eq!(run.stderr_text(), stderr);
}


#[cfg(unix)]
#[test]
fn shutdown_kill_signals_an_isolated_registered_process() {
    let mut command = Command::new("sleep");
    command.arg("30");
    let owner = SupervisedChild::spawn(command);
    let child = &owner.0;
    // A populated private registry is the admission handshake. No elapsed
    // time is used as evidence that a child has registered.
    let registry = Mutex::new(vec![child.clone()]);
    kill_running(&registry);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if lock_child(&child).try_wait().unwrap().is_some() { break; }
        assert!(Instant::now() < deadline, "shutdown signal did not end its child");
        std::thread::sleep(Duration::from_millis(5));
    }
    kill_running(&Mutex::new(Vec::new()));
}

#[test]
fn caller_cancellation_leaves_staged_output_owned_until_later_settlement() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pending-output");
    let staged = Arc::new(StagedOutput::new(path.clone()).unwrap());
    let retained = Arc::downgrade(&staged);
    static CAPACITY: AtomicUsize = AtomicUsize::new(0);
    let capacity = Capacity::reserve_from(&CAPACITY, 1).unwrap();
    let (release, held) = std::sync::mpsc::channel();
    let (settled, receiver) = std::sync::mpsc::channel();
    let owner = std::thread::spawn(move || {
        let _capacity = capacity;
        let _output = staged;
        held.recv().unwrap();
        let _ = settled.send(OwnerEvent::Settled(Err(crate::scanner::CANCELLED.to_string())));
    });
    let cancellation = Arc::new(AtomicBool::new(false));
    let result = wait_for_owner(receiver, cancellation.clone(), &|| true, Duration::from_millis(5));
    assert_eq!(result.err().unwrap(), crate::scanner::CANCELLED);
    assert!(cancellation.load(Ordering::Acquire));
    assert!(retained.upgrade().is_some());
    assert!(path.exists());
    let refusal = Capacity::reserve_from(&CAPACITY, 1).err().unwrap();
    assert!(crate::resource_limits::is_safety_error(&refusal));
    assert_eq!(CAPACITY.load(Ordering::Acquire), 1);
    release.send(()).unwrap();
    owner.join().unwrap();
    assert!(retained.upgrade().is_none());
    assert!(!path.exists());
    assert_eq!(CAPACITY.load(Ordering::Acquire), 0);
    drop(Capacity::reserve_from(&CAPACITY, 1).unwrap());
}

#[cfg(unix)]
#[test]
#[serial_test::serial(subprocess)]
fn streamed_stdout_is_bounded_and_kept_out_of_the_result_buffer() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let staged = Arc::new(StagedOutput::new(root.path().join("output")).unwrap());
    assert_eq!(std::fs::metadata(staged.path()).unwrap().permissions().mode() & 0o777, 0o600);
    let mut command = Command::new("sh");
    command.args(["-c", "printf 'streamed bytes'"]);
    let run = run_bounded_to_file(command, &|| false, staged.clone(), 100).unwrap();
    assert!(run.status_ok);
    assert!(run.stdout.is_empty());
    assert_eq!(std::fs::read(staged.path()).unwrap(), b"streamed bytes");
    assert!(StagedOutput::new(staged.path().to_path_buf()).is_err(), "staging must be exclusive");
    let oversized = Arc::new(StagedOutput::new(root.path().join("oversized")).unwrap());
    let mut command = Command::new("sh");
    command.args(["-c", "printf 'too many bytes'; sleep 30"]);
    let result = run_bounded_to_file(command, &|| false, oversized.clone(), 4);
    assert!(result.err().unwrap().contains("safety limit"));
    assert!(std::fs::metadata(oversized.path()).unwrap().len() <= 4);
}

#[test]
fn output_write_failure_is_signalled_to_the_process_owner() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("read-only-writer");
    std::fs::write(&path, []).unwrap();
    let output = Arc::new(StagedOutput {
        path: path.clone(),
        file: Mutex::new(Some(std::fs::File::open(&path).unwrap())),
        _owner: None,
    });
    let failure = Arc::new(Mutex::new(None));
    let reader = start_reader("stdout", std::io::Cursor::new(b"bytes"), StdoutTarget::File(output, 100),
        Arc::new(AtomicU64::new(0)), Instant::now(), failure.clone()).unwrap();
    assert!(reader.join().unwrap().is_err());
    assert!(failure.lock().unwrap().as_ref().unwrap().contains("write failed"));
    assert!(!path.exists(), "reader closes the file before final output cleanup");
}


#[test]
fn terminal_failure_bounds_caller_wait_without_relabelling_the_reason() {
    let (events, receiver) = std::sync::mpsc::channel();
    events.send(OwnerEvent::Stopping("output write failed".to_string())).unwrap();
    let result = wait_for_owner(receiver, Arc::new(AtomicBool::new(false)), &|| false, Duration::ZERO);
    assert_eq!(result.err().unwrap(), "output write failed: subprocess settlement pending");
    // Retaining the sender represents the owner still settling its effect.
    drop(events);
}

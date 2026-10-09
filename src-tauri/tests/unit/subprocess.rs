use super::*;

#[test]
#[serial_test::serial(subprocess_registry)]
fn a_contended_registry_cannot_block_the_force_exit_signal() {
    let held = RUNNING.lock().unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || { signal_all_running_for_exit(); done.send(()).unwrap(); });
    let returned = finished.recv_timeout(Duration::from_secs(2)).is_ok();
    drop(held);
    worker.join().unwrap();
    assert!(returned, "a held registry must not extend forced exit");
}

#[cfg(unix)]
#[test]
#[serial_test::serial(subprocess_registry)]
fn a_deadline_signal_ends_the_live_group_and_leaves_reaping_to_its_owner() {
    struct OwnedChild(Arc<Mutex<OwnedProcess>>);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let mut child = lock_child(&self.0);
            if matches!(child.try_wait(), Ok(None)) { kill_owned(&mut child); }
        }
    }
    let mut command = Command::new("sleep");
    command.arg("60").process_group(0);
    let owner = OwnedChild(Arc::new(Mutex::new(OwnedProcess::new(command.spawn().unwrap()))));
    let _registration = register_running(owner.0.clone());
    signal_all_running_for_exit();
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

#[test]
#[serial_test::serial(subprocess_registry)]
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

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
    struct OwnedChild(Arc<Mutex<std::process::Child>>);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let mut child = lock_child(&self.0);
            if matches!(child.try_wait(), Ok(None)) { kill_owned(&mut child); }
        }
    }
    let mut command = Command::new("sleep");
    command.arg("60").process_group(0);
    let owner = OwnedChild(Arc::new(Mutex::new(command.spawn().unwrap())));
    let _registration = register_running(owner.0.clone());
    signal_all_running_for_exit();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if lock_child(&owner.0).try_wait().unwrap().is_some() { break; }
        assert!(Instant::now() < deadline, "deadline signal did not end the child");
        std::thread::sleep(Duration::from_millis(5));
    }
}

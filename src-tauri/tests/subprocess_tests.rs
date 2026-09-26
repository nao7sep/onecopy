// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

// `sleep` is not a portable command name on Windows; the process-registry
// kill path itself is platform-independent (`std::process::Child`), so one
// platform's coverage is enough (Phase 5's exit-join task).
#[cfg(unix)]
#[test]
fn kill_all_running_ends_a_subprocess_its_own_thread_cannot_reach() {
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("30");
    let handle = std::thread::spawn(move || {
        onecopy_lib::subprocess::run_bounded_idle(
            cmd,
            &|| false,
            std::time::Duration::from_secs(60),
        )
    });

    // Give the child time to spawn and register itself with the exit watchdog's
    // registry before this thread — standing in for the exit watchdog — kills it.
    std::thread::sleep(std::time::Duration::from_millis(300));
    onecopy_lib::subprocess::kill_all_running();

    let started = std::time::Instant::now();
    let _ = handle.join().unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "kill_all_running must end the wait promptly instead of after the full sleep"
    );
}

#[cfg(unix)]
#[test]
fn kill_all_running_is_a_harmless_no_op_when_nothing_is_running() {
    onecopy_lib::subprocess::kill_all_running();
}

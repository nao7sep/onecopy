use super::*;
use std::time::{Duration, Instant};

#[test]
fn a_guardian_ends_its_group_without_signalling_an_unrelated_child() {
    let mut unrelated = Command::new("sleep").arg("60").spawn().unwrap();
    let mut command = Command::new("sleep");
    command.arg("60");
    let mut guard = prepare(&mut command).unwrap();
    let mut tool = command.spawn().unwrap();
    guard.signal();
    let deadline = Instant::now() + Duration::from_secs(5);
    while tool.try_wait().unwrap().is_none() || !guard.try_settle().unwrap() {
        assert!(Instant::now() < deadline, "guardian left its tool alive");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(unrelated.try_wait().unwrap().is_none());
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[test]
#[ignore = "subprocess fixture: invoked only by abrupt_parent_exit_ends_the_owned_tool"]
fn abrupt_parent_fixture() {
    let path = std::env::var_os("ONECOPY_PARENT_EXIT_FIXTURE").expect("fixture directory");
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "echo $$ > \"$1\"; exec sleep 60", "fixture"])
        .arg(std::path::PathBuf::from(path).join("tool.pid"));
    crate::subprocess::run_bounded(command, &|| false).unwrap();
}

#[test]
fn abrupt_parent_exit_ends_the_owned_tool() {
    let dir = tempfile::tempdir().unwrap();
    let mut parent = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "subprocess_supervisor::tests::abrupt_parent_fixture", "--ignored", "--nocapture"])
        .env("ONECOPY_PARENT_EXIT_FIXTURE", dir.path())
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(dir.path().join("tool.pid")) {
            if let Ok(pid) = text.trim().parse::<i32>() { break pid; }
        }
        assert!(parent.try_wait().unwrap().is_none(), "fixture failed before starting its tool");
        assert!(Instant::now() < deadline, "tool did not announce readiness");
        std::thread::sleep(Duration::from_millis(10));
    };
    parent.kill().unwrap();
    parent.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Signal zero only observes the exact PID the fixture created.
        if unsafe { libc::kill(pid, 0) } != 0 { break; }
        if Instant::now() >= deadline {
            unsafe { libc::kill(pid, libc::SIGKILL); }
            panic!("abrupt app exit left the managed tool alive");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

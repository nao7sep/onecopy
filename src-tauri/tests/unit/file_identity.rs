// Private helpers behind `file_identity::is_abandoned_leftover`'s
// process-global public surface; see the `mod tests` stub in
// `src/file_identity.rs` for why these are exercised here instead of through
// the public API.

use super::*;

fn owner_name(fingerprint: &str, pid: u32) -> String {
    format!(
        "{STAGE_PREFIX}{fingerprint}-{:0width$}-nanoid{PRIVATE_TMP_SUFFIX}",
        pid,
        width = PID_DECIMAL_LEN
    )
}

fn exited_pid() -> u32 {
    let mut child = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "exit", "0"])
            .spawn()
            .unwrap()
    } else {
        std::process::Command::new("true").spawn().unwrap()
    };
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn two_homes_on_one_host_never_collide() {
    assert_ne!(
        fingerprint_for("installation-a", "shared-host"),
        fingerprint_for("installation-b", "shared-host"),
        "two different installation ids on the same host must fingerprint differently"
    );
}

#[test]
fn a_cloned_home_on_a_different_host_never_collides() {
    // Simulates the exact regression: a data root (installation id included)
    // duplicated onto a second computer. The installation id alone would
    // collide; the host name must still tell the two apart.
    assert_ne!(
        fingerprint_for("same-installation-id", "host-one"),
        fingerprint_for("same-installation-id", "host-two"),
        "the same installation id on two different hosts must fingerprint differently"
    );
}

#[test]
fn unsettled_process_sweeps_nothing() {
    let fingerprint = "a".repeat(HOME_FINGERPRINT_HEX_LEN);
    let dead_pid = exited_pid();
    let path = owner_name(&fingerprint, dead_pid);

    assert!(
        !is_abandoned_leftover_against(Path::new(&path), None),
        "a process with no proven fingerprint of its own can prove nothing is abandoned"
    );
}

#[test]
fn this_installations_dead_pid_file_is_swept() {
    let fingerprint = "a".repeat(HOME_FINGERPRINT_HEX_LEN);
    let dead_pid = exited_pid();
    let dead_path = owner_name(&fingerprint, dead_pid);
    assert!(
        is_abandoned_leftover_against(Path::new(&dead_path), Some(&fingerprint)),
        "this installation's own dead-pid leftover is provably abandoned"
    );

    let live_path = owner_name(&fingerprint, std::process::id());
    assert!(
        !is_abandoned_leftover_against(Path::new(&live_path), Some(&fingerprint)),
        "this installation's own live-pid file must never look abandoned"
    );

    let other_fingerprint = "b".repeat(HOME_FINGERPRINT_HEX_LEN);
    assert!(
        !is_abandoned_leftover_against(Path::new(&dead_path), Some(&other_fingerprint)),
        "a different installation's dead-pid-looking file is never treated as this one's"
    );
}

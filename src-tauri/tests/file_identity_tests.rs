use onecopy_lib::file_identity::*;

#[test]
fn private_cleanup_preserves_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stage.tmp");
    let held = dir.path().join("held.tmp");
    std::fs::write(&path, b"ours").unwrap();
    let ours = std::fs::File::open(&path).unwrap();
    std::fs::rename(&path, &held).unwrap();
    std::fs::write(&path, b"winner").unwrap();

    remove_private_if_owned(&path, &ours);

    assert_eq!(std::fs::read(&path).unwrap(), b"winner");
    assert_eq!(std::fs::read(&held).unwrap(), b"ours");
}

#[test]
fn physical_claim_rejects_and_restores_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stage.tmp");
    let ours = dir.path().join("ours.tmp");
    std::fs::write(&path, b"ours").unwrap();
    let descriptor = std::fs::File::open(&path).unwrap();
    std::fs::rename(&path, &ours).unwrap();
    std::fs::write(&path, b"winner").unwrap();

    assert!(claim_private(&path, &descriptor).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"winner");
    assert_eq!(std::fs::read(&ours).unwrap(), b"ours");
}

#[cfg(unix)]
#[test]
fn nofollow_regular_open_rejects_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.bin");
    let link = dir.path().join("link.bin");
    std::fs::write(&real, b"bytes").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    assert!(open_regular_nofollow(&link).is_err());
    assert_eq!(std::fs::read(&real).unwrap(), b"bytes");
}

// `.onecopy-stage-<16 hex home fingerprint>-<10 digit pid>-<nanoid>.tmp`; see
// `file_identity::private_stage_file_name`. This test binary never settles
// `paths::data_root`, so every name this process mints carries the fixed
// "unsettled" placeholder fingerprint (16 zeros) — and, because this process's
// own fingerprint is unproven, `is_abandoned_leftover` never treats anything
// as abandoned here, not even a file carrying that same placeholder and a
// dead pid (see `unsettled_process_never_sweeps_even_its_own_dead_pid_file`
// below). The settled-home sweep decision itself is exercised at the unit
// level in `tests/unit/file_identity.rs`, which is not pinned to one
// process-global data root.
const STAGE_PREFIX_LEN: usize = ".onecopy-stage-".len();
const HOME_FIELD_LEN: usize = 16;
const PID_FIELD_LEN: usize = 10;

/// Replaces the process-id field of a name `private_stage_file_name` minted,
/// keeping its (unsettled-placeholder) home fingerprint untouched.
fn with_pid(name: &str, pid: u32) -> String {
    let pid_start = STAGE_PREFIX_LEN + HOME_FIELD_LEN + 1;
    let pid_end = pid_start + PID_FIELD_LEN;
    format!("{}{pid:0width$}{}", &name[..pid_start], &name[pid_end..], width = PID_FIELD_LEN)
}

/// Replaces the home-fingerprint field, keeping the process-id field intact.
fn with_foreign_home(name: &str) -> String {
    let home_start = STAGE_PREFIX_LEN;
    let home_end = home_start + HOME_FIELD_LEN;
    let foreign = "f".repeat(HOME_FIELD_LEN);
    format!("{}{foreign}{}", &name[..home_start], &name[home_end..])
}

/// Spawns a trivial child process and waits for it to exit, returning a pid
/// that is guaranteed not to be running any more.
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
fn unsettled_process_never_sweeps_even_its_own_dead_pid_file() {
    let base = private_stage_file_name().unwrap();
    assert!(is_private_tmp_name(std::path::Path::new(&base)));

    let live = with_pid(&base, std::process::id());
    assert!(
        !is_abandoned_leftover(std::path::Path::new(&live)),
        "a live process's file must never look abandoned"
    );

    let dead_pid = exited_pid();
    let dead = with_pid(&base, dead_pid);
    assert!(
        !is_abandoned_leftover(std::path::Path::new(&dead)),
        "an unsettled process has no proven identity yet, so it sweeps nothing, \
         not even a dead-pid file carrying its own unsettled placeholder"
    );

    let foreign_home_dead = with_foreign_home(&dead);
    assert!(
        !is_abandoned_leftover(std::path::Path::new(&foreign_home_dead)),
        "a different application home's file is never removed, even with a dead-looking pid"
    );
}

#[test]
fn sweep_removes_nothing_while_unsettled() {
    let dir = tempfile::tempdir().unwrap();
    let base = private_stage_file_name().unwrap();
    let dead_pid = exited_pid();

    let live_path = dir.path().join(with_pid(&base, std::process::id()));
    let dead_path = dir.path().join(with_pid(&base, dead_pid));
    let foreign_path = dir.path().join(with_foreign_home(&with_pid(&base, dead_pid)));
    let ordinary_path = dir.path().join("photo.jpg");
    for path in [&live_path, &dead_path, &foreign_path, &ordinary_path] {
        std::fs::write(path, b"x").unwrap();
    }

    sweep_private_tmp_leftovers(dir.path());

    assert!(live_path.exists(), "a live process's file survives the sweep");
    assert!(
        dead_path.exists(),
        "this unsettled process's own dead-pid-looking file survives too: \
         it has no proven fingerprint to sweep with"
    );
    assert!(foreign_path.exists(), "a different home's file survives the sweep");
    assert!(ordinary_path.exists(), "ordinary content is never touched");
}

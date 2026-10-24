// Private helpers behind `file_identity::is_abandoned_leftover`'s
// process-global public surface. They are exercised here instead of through
// the public API because every case needs that process-wide settled data root
// and fingerprint set differently, which one test binary cannot do.

use super::*;

#[test]
fn a_source_version_rejects_a_new_inode_with_the_same_size_and_time() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    std::fs::write(&source, b"original").unwrap();
    let (mut file, _) = open_regular_nofollow(&source).unwrap();
    let version = FileVersion::of(&mut file).unwrap();
    assert!(version.matches_path(&source).unwrap());
    std::fs::rename(&source, dir.path().join("old-source.bin")).unwrap();
    std::fs::write(&source, b"changed!").unwrap();
    std::fs::OpenOptions::new().write(true).open(&source).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(version.modified)).unwrap();
    assert!(!version.matches_path(&source).unwrap());
}

#[test]
fn a_source_version_rejects_an_inplace_edit_with_a_changed_time() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    std::fs::write(&source, b"original").unwrap();
    let (mut file, _) = open_regular_nofollow(&source).unwrap();
    let version = FileVersion::of(&mut file).unwrap();
    std::fs::write(&source, b"changed!").unwrap();
    std::fs::OpenOptions::new().write(true).open(&source).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(version.modified + std::time::Duration::from_secs(60))).unwrap();
    assert!(!version.matches_path(&source).unwrap());
}

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

#[test]
fn sweep_removes_only_this_installations_dead_leftover() {
    // The mechanism `sweep_private_tmp_leftovers` delegates to, exercised
    // with a proven fingerprint injected directly: a real process-global
    // settled data root cannot be set up per case in one test binary, so
    // this is where "the dead leftover is actually removed from disk" is
    // proven, matching what a destination-folder sweep does once this
    // process's identity is settled. `operations_tests.rs`'s
    // `an_operation_sweeps_only_its_own_dead_leftover_from_the_destination_folder`
    // proves the complementary, always-true-in-that-process half: an
    // unsettled process sweeps nothing, not even its own dead-pid-looking
    // leftover.
    let dir = tempfile::tempdir().unwrap();
    let fingerprint = "a".repeat(HOME_FINGERPRINT_HEX_LEN);
    let dead_pid = exited_pid();
    let dead = dir.path().join(owner_name(&fingerprint, dead_pid));
    let live = dir.path().join(owner_name(&fingerprint, std::process::id()));
    let other_fingerprint = "b".repeat(HOME_FINGERPRINT_HEX_LEN);
    let foreign = dir.path().join(owner_name(&other_fingerprint, dead_pid));
    let ordinary = dir.path().join("photo.jpg");
    for path in [&dead, &live, &foreign, &ordinary] {
        std::fs::write(path, b"leftover").unwrap();
    }

    sweep_private_tmp_leftovers_against(dir.path(), Some(&fingerprint));

    assert!(!dead.exists(), "this installation's dead-process leftover is swept");
    assert!(live.exists(), "a live process's own file is never removed");
    assert!(foreign.exists(), "a different installation's file is never removed");
    assert!(ordinary.exists(), "ordinary content is never touched");
}

#[test]
fn malformed_private_names_prove_no_owner_and_survive_sweeping() {
    let directory = tempfile::tempdir().unwrap();
    let fingerprint = "a".repeat(HOME_FINGERPRINT_HEX_LEN);
    for prefix in [STAGE_PREFIX, CLAIM_PREFIX] {
        for character in ["é", "🙂"] {
            for position in 0..HOME_FINGERPRINT_HEX_LEN {
                let field = format!("{}{character}{}", "a".repeat(position), "a".repeat(HOME_FINGERPRINT_HEX_LEN - position - 1));
                let name = format!("{prefix}{field}-0000000001-x.tmp");
                assert!(parse_private_tmp_owner(&name).is_none(), "{name}");
                std::fs::write(directory.path().join(name), b"preserve").unwrap();
            }
            for position in 0..PID_DECIMAL_LEN {
                let field = format!("{}{character}{}", "0".repeat(position), "0".repeat(PID_DECIMAL_LEN - position - 1));
                let name = format!("{prefix}{fingerprint}-{field}-x.tmp");
                assert!(parse_private_tmp_owner(&name).is_none(), "{name}");
                std::fs::write(directory.path().join(name), b"preserve").unwrap();
            }
        }
        for field in ["+000000001", "000000001", "00000000001", "9999999999", ""] {
            assert!(parse_private_tmp_owner(&format!("{prefix}{fingerprint}-{field}-x.tmp")).is_none());
        }
        let valid = parse_private_tmp_owner(&format!("{prefix}{fingerprint}-0000000001-x.tmp")).unwrap();
        assert_eq!(valid.home_fingerprint, fingerprint);
        assert_eq!(valid.pid, 1);
    }
    let count = std::fs::read_dir(directory.path()).unwrap().count();
    sweep_private_tmp_leftovers_against(directory.path(), Some(&fingerprint));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), count);
    for entry in std::fs::read_dir(directory.path()).unwrap() {
        assert_eq!(std::fs::read(entry.unwrap().path()).unwrap(), b"preserve");
    }
}

#[test]
fn physical_copy_keys_keep_volume_and_full_file_identity() {
    assert_eq!(physical_copy_key(3, 7, 0), None);
    assert_eq!(physical_copy_key(3, 7, 1), None);
    assert_eq!(physical_copy_key(3, 7, 2), Some("3:7".to_string()));
    assert_eq!(physical_copy_key(3, 7, 2), physical_copy_key(3, 7, 9));
    assert_ne!(physical_copy_key(3, 7, 2), physical_copy_key(4, 7, 2));
    assert_ne!(physical_copy_key(3, 7, 2), physical_copy_key(3, (1u64 << 32) + 7, 2));
}

#[cfg(any(unix, windows))]
#[test]
fn native_link_identity_changes_when_a_second_name_is_added_and_removed() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first.bin");
    let second = temp.path().join("second.bin");
    let separate = temp.path().join("separate.bin");
    std::fs::write(&first, b"same bytes").unwrap();
    std::fs::write(&separate, b"same bytes").unwrap();
    let key = |path: &Path| hard_link_key(path, &std::fs::metadata(path).unwrap()).unwrap();
    assert_eq!(key(&first), None);
    std::fs::hard_link(&first, &second).unwrap();
    let shared = key(&first).expect("two names share one physical file");
    assert_eq!(key(&second), Some(shared));
    assert_eq!(key(&separate), None);
    std::fs::remove_file(&second).unwrap();
    assert_eq!(key(&first), None);
}

#[cfg(windows)]
#[test]
fn failed_windows_link_identity_is_not_treated_as_an_independent_copy() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gone.bin");
    std::fs::write(&path, b"gone").unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(hard_link_key(&path, &metadata).unwrap_err().kind(), std::io::ErrorKind::NotFound);
}

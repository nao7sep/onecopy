#[cfg(unix)]
use std::process::Command;

use onecopy_lib::resource_limits;
#[cfg(unix)]
use onecopy_lib::subprocess;

#[test]
fn oversized_bitmap_is_rejected_before_pixel_allocation() {
    // Minimal BMP header declaring a 100,000 × 100,000 24-bit image. The
    // decoder can read its dimensions from the header, then the allocation
    // ceiling must reject it before absent pixel bytes matter.
    let mut bytes = vec![0u8; 54];
    bytes[0..2].copy_from_slice(b"BM");
    bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
    bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
    bytes[18..22].copy_from_slice(&30_000i32.to_le_bytes());
    bytes[22..26].copy_from_slice(&30_000i32.to_le_bytes());
    bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&24u16.to_le_bytes());

    let error = resource_limits::decode_bytes(&bytes).unwrap_err();
    let expected_limit = resource_limits::MAX_DECODE_ALLOC / 1024 / 1024;
    assert!(
        error.contains(&format!("{expected_limit} MiB safety limit")),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
#[serial_test::serial(subprocess)]
fn subprocess_stdout_stops_at_the_declared_ceiling() {
    let mut command = Command::new("sh");
    command.args(["-c", "head -c 4096 /dev/zero"]);
    let error = match subprocess::run_bounded_output(command, &|| false, 1024) {
        Ok(_) => panic!("oversized stdout unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(error.contains("output exceeded"), "{error}");
}

#[cfg(unix)]
#[test]
#[serial_test::serial(subprocess)]
fn subprocess_stdout_has_a_default_ceiling() {
    let mut command = Command::new("sh");
    command.arg("-c").arg(format!("head -c {} /dev/zero", subprocess::STDOUT_BYTES + 1));
    let error = match subprocess::run_bounded(command, &|| false) {
        Ok(_) => panic!("oversized default stdout unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(error.contains("output exceeded"), "{error}");
}

#[cfg(unix)]
#[test]
#[serial_test::serial(subprocess)]
fn subprocess_stderr_has_a_fixed_ceiling() {
    let mut command = Command::new("sh");
    command.arg("-c").arg(format!("head -c {} /dev/zero >&2", subprocess::STDERR_BYTES + 1));
    let error = match subprocess::run_bounded(command, &|| false) {
        Ok(_) => panic!("oversized stderr unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(error.contains("stderr exceeded"), "{error}");
}

#[cfg(unix)]
#[test]
#[serial_test::serial(subprocess)]
fn cancelling_a_subprocess_kills_its_owned_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("descendant.pid");
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("sleep 30 & echo $! > \"$1\"; wait")
        .arg("onecopy-subprocess-test")
        .arg(&pid_file);
    let recorded = || std::fs::read_to_string(&pid_file).ok()
        .and_then(|text| text.trim().parse::<i32>().ok());
    let watchdog = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let result = subprocess::run_bounded(command, &|| {
        recorded().is_some() || std::time::Instant::now() >= watchdog
    });
    assert!(result.is_err());
    let pid = recorded().expect("the owned child must publish its PID before cancellation");
    let settlement = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        // SAFETY: signal 0 performs only a liveness probe of the recorded child.
        if unsafe { libc::kill(pid, 0) } != 0 { break; }
        assert!(std::time::Instant::now() < settlement, "owned descendant {pid} survived cancellation");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

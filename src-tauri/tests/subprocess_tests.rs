// Public bounded command behavior; private shutdown registries are tested
// beside their owning module rather than killing parallel tests' processes.

#[cfg(unix)]
#[test]
#[serial_test::serial(subprocess)]
fn bounded_command_captures_output_and_exit_status() {
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "printf stdout; printf stderr >&2; exit 3"]);
    let result = onecopy_lib::subprocess::run_bounded(command, &|| false).unwrap();
    assert!(!result.status_ok);
    assert_eq!(result.stdout, b"stdout");
    assert_eq!(result.stderr, "stderr");
}

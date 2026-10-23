//! Unix lifetime guardian. Its process group exists before the tool can exec,
//! and remains owned until the guardian is reaped. Closing the parent's pipe
//! (including abrupt parent exit) ends that exact group. The system shell runs
//! only builtins; it neither launches the app UI nor needs a bundled helper.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};

pub(super) struct Guard {
    child: Child,
    lifetime: Option<ChildStdin>,
}

impl Guard {
    pub(super) fn signal(&mut self) {
        // EOF is the termination request. The tool cannot inherit this write
        // end: Rust's pipe is close-on-exec, and the guardian owns only stdin.
        self.lifetime.take();
    }

    pub(super) fn try_settle(&mut self) -> io::Result<bool> {
        self.child.try_wait().map(|status| status.is_some())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.signal();
        // Normal ownership reaps explicitly outside shared locks. This fallback
        // handles a failed tool spawn; the guardian never accesses a user volume.
        if let Err(error) = self.child.wait() {
            crate::logging::warn("subprocess guardian reap failed", serde_json::json!({"error": error.to_string()}));
        }
    }
}

pub(super) fn prepare(command: &mut Command) -> io::Result<Guard> {
    let mut child = Command::new("/bin/sh")
        .env_clear()
        .arg("-c")
        .arg("while IFS= read -r line; do :; done; kill -KILL -- -$$")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let lifetime = child.stdin.take();
    let group = child.id() as libc::pid_t;
    // SAFETY: only the async-signal-safe setpgid syscall runs after fork. The
    // guardian already owns this group and is not reaped until tool settlement.
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, group) == 0 { Ok(()) }
            else { Err(io::Error::last_os_error()) }
        });
    }
    Ok(Guard { child, lifetime })
}

#[cfg(test)]
#[path = "../tests/unit/subprocess_supervisor.rs"]
mod tests;

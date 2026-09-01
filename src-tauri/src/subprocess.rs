//! Bounded subprocess execution for the managed tools.
//!
//! Every ffmpeg invocation used to be a plain blocking `output()`/`status()`,
//! which meant two things the app could not recover from: a hung ffmpeg hung
//! the scan AND the quit forever, and the derive thread could not see the
//! cancel flag while a child ran, so quitting during a large video waited for
//! that video to finish.
//!
//! Both are fixed by watching the child rather than blocking on it. Following
//! tapebox's `spawn.ts`, the bound is an IDLE timeout rather than a wall-clock
//! one: a legitimately slow job on a 4 GB video keeps producing output and is
//! left alone, while a genuinely stuck one is killed. A wall-clock limit
//! cannot tell those apart and would kill real work on big files.
//!
//! Errors carry a bounded TAIL of recent output — enough to diagnose the one
//! file that failed, not an ffmpeg essay carried into a database column.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// No output at all for this long means stuck, not slow.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// How much recent stderr an error carries.
pub const TAIL_BYTES: usize = 2000;
pub const STDOUT_BYTES: usize = 1024 * 1024;
pub const STDERR_BYTES: usize = 1024 * 1024;

/// How often the watcher wakes to check liveness, idleness and cancellation.
const POLL: Duration = Duration::from_millis(100);

pub struct Run {
    pub status_ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Run {
    /// The last `TAIL_BYTES` of stderr, for an error message.
    pub fn stderr_tail(&self) -> String {
        let s = self.stderr.trim();
        if s.len() <= TAIL_BYTES {
            return s.to_string();
        }
        // Cut on a char boundary — stderr is arbitrary bytes lossily decoded.
        let start = s
            .char_indices()
            .nth(s.chars().count().saturating_sub(TAIL_BYTES))
            .map(|(i, _)| i)
            .unwrap_or(0);
        format!("…{}", &s[start..])
    }
}

/// Runs `command` to completion, killing it if it goes idle or if `cancelled`
/// starts returning true.
///
/// `cancelled` is polled rather than captured once so a quit mid-derive kills
/// the child instead of waiting for it — the difference between an app that
/// closes and one that appears to hang.
pub fn run_bounded(command: Command, cancelled: &dyn Fn() -> bool) -> Result<Run, String> {
    run_bounded_idle_output(command, cancelled, IDLE_TIMEOUT, STDOUT_BYTES)
}

/// `run_bounded` with a hard stdout ceiling. Binary-producing commands use
/// this so a malformed or unexpectedly large stream cannot grow a `Vec`
/// until the process is out of memory.
pub fn run_bounded_output(
    command: Command,
    cancelled: &dyn Fn() -> bool,
    max_stdout: usize,
) -> Result<Run, String> {
    run_bounded_idle_output(command, cancelled, IDLE_TIMEOUT, max_stdout)
}

/// `run_bounded` with a caller-chosen idle bound, for work whose healthy runtime
/// is far below the media default — a `-version` probe answers in milliseconds,
/// and letting a wedged one hold a status read for the full 120s would look like
/// a hang.
pub fn run_bounded_idle(
    command: Command,
    cancelled: &dyn Fn() -> bool,
    idle_timeout: Duration,
) -> Result<Run, String> {
    run_bounded_idle_output(command, cancelled, idle_timeout, STDOUT_BYTES)
}

fn run_bounded_idle_output(
    mut command: Command,
    cancelled: &dyn Fn() -> bool,
    idle_timeout: Duration,
    max_stdout: usize,
) -> Result<Run, String> {
    #[cfg(windows)]
    // Managed command-line tools must remain app-owned background work. A
    // GUI parent otherwise causes Windows to allocate a visible console for
    // ffmpeg, covering OneCopy for the lifetime of a long media operation.
    command.creation_flags(CREATE_NO_WINDOW);

    #[cfg(unix)]
    // SAFETY: this closure runs in the child after fork and before exec. It
    // performs one async-signal-safe syscall and returns only its OS error.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    // Both pipes are drained on their own threads. This is not just tidiness:
    // a child that fills a pipe buffer nobody reads blocks forever, which
    // would be a hang this function exists to prevent.
    let last_output = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let stdout_buf = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::new()));
    let stdout_overflow = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stderr_overflow = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let mut readers = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let buf = Arc::clone(&stdout_buf);
        let seen = Arc::clone(&last_output);
        let overflow = Arc::clone(&stdout_overflow);
        let reader = std::thread::Builder::new()
            .name("onecopy-subprocess-stdout".to_string())
            .spawn(move || -> Result<(), String> {
                let mut chunk = [0u8; 64 * 1024];
                loop {
                    let n = out.read(&mut chunk).map_err(|error| error.to_string())?;
                    if n == 0 {
                        return Ok(());
                    }
                    seen.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    if let Ok(mut b) = buf.lock() {
                        if b.len().saturating_add(n) > max_stdout {
                            overflow.store(true, Ordering::Relaxed);
                        } else if !overflow.load(Ordering::Relaxed) {
                            b.extend_from_slice(&chunk[..n]);
                        }
                    }
                }
            });
        match reader {
            Ok(reader) => readers.push(reader),
            Err(error) => {
                kill_owned(&mut child);
                return Err(format!("could not start subprocess stdout reader: {error}"));
            }
        }
    }
    if let Some(mut err) = child.stderr.take() {
        let buf = Arc::clone(&stderr_buf);
        let seen = Arc::clone(&last_output);
        let overflow = Arc::clone(&stderr_overflow);
        let reader = std::thread::Builder::new()
            .name("onecopy-subprocess-stderr".to_string())
            .spawn(move || -> Result<(), String> {
                let mut chunk = [0u8; 16 * 1024];
                loop {
                    let n = err.read(&mut chunk).map_err(|error| error.to_string())?;
                    if n == 0 {
                        return Ok(());
                    }
                    seen.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    if let Ok(mut b) = buf.lock() {
                        if b.len().saturating_add(n) > STDERR_BYTES {
                            overflow.store(true, Ordering::Relaxed);
                        } else if !overflow.load(Ordering::Relaxed) {
                            b.extend_from_slice(&chunk[..n]);
                        }
                    }
                }
            });
        match reader {
            Ok(reader) => readers.push(reader),
            Err(error) => {
                kill_owned(&mut child);
                for reader in readers {
                    match reader.join() {
                        Ok(Ok(())) => {}
                        Ok(Err(read_error)) => crate::logging::warn(
                            "subprocess reader cleanup failed",
                            serde_json::json!({
                                "error": { "message": read_error },
                            }),
                        ),
                        Err(payload) => crate::logging::warn(
                            "subprocess reader cleanup panicked",
                            serde_json::json!({
                                "error": {
                                    "message": crate::failure_runtime::panic_message(payload),
                                },
                            }),
                        ),
                    }
                }
                return Err(format!("could not start subprocess stderr reader: {error}"));
            }
        }
    }

    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Ok(None) => {}
            Err(e) => break Err(e.to_string()),
        }
        if cancelled() {
            kill_owned(&mut child);
            break Err(crate::scanner::CANCELLED.to_string());
        }
        if stdout_overflow.load(Ordering::Relaxed) {
            kill_owned(&mut child);
            break Err(format!(
                "subprocess output exceeded the {} MiB safety limit",
                max_stdout / 1024 / 1024
            ));
        }
        if stderr_overflow.load(Ordering::Relaxed) {
            kill_owned(&mut child);
            break Err("subprocess stderr exceeded the 1 MiB safety limit".to_string());
        }
        let idle_ms = (started.elapsed().as_millis() as u64)
            .saturating_sub(last_output.load(Ordering::Relaxed));
        if idle_ms > idle_timeout.as_millis() as u64 {
            kill_owned(&mut child);
            break Err(format!(
                "no output for {}s — killed as stuck",
                idle_timeout.as_secs()
            ));
        }
        std::thread::sleep(POLL);
    };

    // Joining after the child is gone: the readers end when their pipes close.
    let mut reader_failure = None;
    for reader in readers {
        let result = reader
            .join()
            .map_err(crate::failure_runtime::panic_message)
            .and_then(|result| result);
        if reader_failure.is_none() {
            reader_failure = result.err();
        }
    }
    if let Some(error) = reader_failure {
        return Err(error);
    }
    let stdout = stdout_buf
        .lock()
        .map_err(|_| "subprocess stdout buffer is unavailable".to_string())?
        .clone();
    let stderr_bytes = stderr_buf
        .lock()
        .map_err(|_| "subprocess stderr buffer is unavailable".to_string())?
        .clone();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

    if stdout_overflow.load(Ordering::Relaxed) {
        return Err(format!(
            "subprocess output exceeded the {} MiB safety limit",
            max_stdout / 1024 / 1024
        ));
    }
    if stderr_overflow.load(Ordering::Relaxed) {
        return Err("subprocess stderr exceeded the 1 MiB safety limit".to_string());
    }

    match outcome {
        Ok(status_ok) => Ok(Run { status_ok, stdout, stderr }),
        Err(reason) => {
            let run = Run { status_ok: false, stdout, stderr };
            let tail = run.stderr_tail();
            Err(if tail.is_empty() {
                reason
            } else {
                format!("{reason}: {tail}")
            })
        }
    }
}

#[cfg(unix)]
fn kill_owned(child: &mut std::process::Child) {
    // The child created its own process group before exec, so the negative PID
    // reaches ffmpeg and anything it spawned without touching unrelated app
    // or external-player processes.
    // SAFETY: kill has no memory preconditions; the PID is the owned child.
    let killed = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    if killed != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            crate::logging::warn(
                "subprocess group termination failed",
                serde_json::json!({ "error": { "message": error.to_string() } }),
            );
        }
    }
    if let Err(error) = child.wait() {
        crate::logging::warn(
            "subprocess reap failed",
            serde_json::json!({ "error": { "message": error.to_string() } }),
        );
    }
}

#[cfg(not(unix))]
fn kill_owned(child: &mut std::process::Child) {
    if let Err(error) = child.kill() {
        if error.kind() != std::io::ErrorKind::InvalidInput {
            crate::logging::warn(
                "subprocess termination failed",
                serde_json::json!({ "error": { "message": error.to_string() } }),
            );
        }
    }
    if let Err(error) = child.wait() {
        crate::logging::warn(
            "subprocess reap failed",
            serde_json::json!({ "error": { "message": error.to_string() } }),
        );
    }
}

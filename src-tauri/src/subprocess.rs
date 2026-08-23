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

/// No output at all for this long means stuck, not slow.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// How much recent stderr an error carries.
pub const TAIL_BYTES: usize = 2000;

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
    run_bounded_idle(command, cancelled, IDLE_TIMEOUT)
}

/// `run_bounded` with a caller-chosen idle bound, for work whose healthy runtime
/// is far below the media default — a `-version` probe answers in milliseconds,
/// and letting a wedged one hold a status read for the full 120s would look like
/// a hang.
pub fn run_bounded_idle(
    mut command: Command,
    cancelled: &dyn Fn() -> bool,
    idle_timeout: Duration,
) -> Result<Run, String> {
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

    let mut readers = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let buf = Arc::clone(&stdout_buf);
        let seen = Arc::clone(&last_output);
        readers.push(std::thread::spawn(move || {
            let mut chunk = [0u8; 64 * 1024];
            while let Ok(n) = out.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                seen.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                if let Ok(mut b) = buf.lock() {
                    b.extend_from_slice(&chunk[..n]);
                }
            }
        }));
    }
    if let Some(mut err) = child.stderr.take() {
        let buf = Arc::clone(&stderr_buf);
        let seen = Arc::clone(&last_output);
        readers.push(std::thread::spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            while let Ok(n) = err.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                seen.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                if let Ok(mut b) = buf.lock() {
                    b.extend_from_slice(&chunk[..n]);
                }
            }
        }));
    }

    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Ok(None) => {}
            Err(e) => break Err(e.to_string()),
        }
        if cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            break Err(crate::scanner::CANCELLED.to_string());
        }
        let idle_ms = started.elapsed().as_millis() as u64
            - last_output.load(Ordering::Relaxed);
        if idle_ms > idle_timeout.as_millis() as u64 {
            let _ = child.kill();
            let _ = child.wait();
            break Err(format!(
                "no output for {}s — killed as stuck",
                idle_timeout.as_secs()
            ));
        }
        std::thread::sleep(POLL);
    };

    // Joining after the child is gone: the readers end when their pipes close.
    for reader in readers {
        let _ = reader.join();
    }
    let stdout = stdout_buf.lock().map(|b| b.clone()).unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr_buf.lock().map(|b| b.clone()).unwrap_or_default())
        .into_owned();

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

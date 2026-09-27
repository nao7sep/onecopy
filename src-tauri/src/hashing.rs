//! Content hashing, tiered (the indexing pipeline's design): files with a
//! unique size are never content-read; size collisions read a 64 KB head+tail
//! prehash; size+prehash collisions get a full blake3 hash. Only full-hash
//! equality collapses copies — a destructive claim needs certainty — and
//! same-size-same-prehash-different-hash surfaces as the copies-disagree
//! anomaly upstream.
//!
//! `hash_while_copying` is the move/copy-out primitive: it hashes the current
//! source bytes while writing, then reads the private output back before the
//! caller publishes it.

use std::io::{Seek, SeekFrom};
use std::path::Path;

use crate::volume_io::{self, VolumeFile};

/// Head/tail window for the prehash tier (public: the tests and any
/// consumer reasoning about the tier need the exact spec value).
pub const PREHASH_WINDOW: u64 = 64 * 1024;

/// Streaming buffer for full hashing and tee-copying.
const BUF_SIZE: usize = 1024 * 1024;

fn cancelled_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")
}

/// blake3 over the first and last 64 KB (whole file when ≤128 KB). Cheap
/// same-size disambiguation only — never a collapse criterion.
pub fn prehash(path: &Path) -> std::io::Result<String> {
    let mut file = volume_io::open_read(path)?;
    let size = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();

    let head_len = size.min(PREHASH_WINDOW);
    copy_exact_into(&mut file, &mut hasher, head_len)?;

    if size > 2 * PREHASH_WINDOW {
        file.seek(SeekFrom::Start(size - PREHASH_WINDOW))?;
        copy_exact_into(&mut file, &mut hasher, PREHASH_WINDOW)?;
    } else if size > PREHASH_WINDOW {
        // Overlapping windows would double-count; hash the remainder once.
        copy_exact_into(&mut file, &mut hasher, size - PREHASH_WINDOW)?;
    }

    Ok(hasher.finalize().to_hex().to_string())
}

/// Full streaming blake3 of the file's bytes.
pub fn full_hash(path: &Path) -> std::io::Result<String> {
    let mut file = volume_io::open_read(path)?;
    hash_from_current(&mut file, &|| false, &mut |_| {})
}

/// Hashes the rest of `file` in bounded 1 MiB reads. `cancel` is checked
/// between reads and ends a read still waiting on a stalled volume.
fn hash_from_current(
    file: &mut VolumeFile,
    cancel: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64),
) -> std::io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; BUF_SIZE];
    let mut done = 0u64;
    loop {
        if cancel() {
            return Err(cancelled_error());
        }
        let read = file.read_cancellable(&mut buf, Some(cancel))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        done = done.saturating_add(read as u64);
        progress(done);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub(crate) fn full_hash_file_cancellable(
    file: &mut VolumeFile,
    total: u64,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, u64),
) -> std::io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    progress(0, total);
    hash_from_current(file, cancelled, &mut |done| progress(done, total))
}

/// Full hash with caller-owned cancellation. Derived work uses its combined
/// pause/preemption boundary; scanner work keeps using its atomic token.
pub fn full_hash_with_cancel(path: &Path, cancel: &dyn Fn() -> bool) -> std::io::Result<String> {
    let mut file = volume_io::open_read(path)?;
    hash_from_current(&mut file, cancel, &mut |_| {})
}

/// The scan-progress variant reports bytes after each bounded read. The
/// caller owns coalescing and presentation; hashing owns the exact descriptor
/// byte count and therefore is the only honest source for this percentage.
pub fn full_hash_cancellable_with_progress(
    path: &Path,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &dyn Fn(u64, u64),
) -> std::io::Result<String> {
    let mut file = volume_io::open_read(path)?;
    let total = file.metadata()?.len();
    progress(0, total);
    hash_from_current(
        &mut file,
        &|| cancel.load(std::sync::atomic::Ordering::Relaxed),
        &mut |done| progress(done, total),
    )
}

/// Copies `src` to `dst` while hashing the bytes read. Returns (hash, bytes
/// copied, the private output bound to the descriptor that wrote it).
/// not recorded: this writes the user's own media into a destination root —
/// OUTPUT, not app-managed text (data-backup conventions).
/// The destination is created fresh (never clobbering an existing file: the
/// collision policy upstream decides skips/conflicts before this runs) and
/// fsynced before return; publication belongs to the caller, and dropping the
/// returned output unpublished removes it.
pub fn hash_while_copying(
    src: &Path,
    dst: &Path,
) -> std::io::Result<(String, u64, crate::file_identity::PrivateFile)> {
    hash_while_copying_detailed(src, dst, &|| false, &mut |_, _| {}, |_| {})
        .map_err(CopyFailure::into_io)
}

#[cfg(test)]
fn hash_while_copying_with_after_sync(
    src: &Path,
    dst: &Path,
    after_sync: impl FnOnce(&Path),
) -> std::io::Result<(String, u64, crate::file_identity::PrivateFile)> {
    hash_while_copying_detailed(src, dst, &|| false, &mut |_, _| {}, after_sync)
        .map_err(CopyFailure::into_io)
}

/// The destination-batch variant may stop while the bytes are still private.
/// Publication is a later step, so cancellation here never creates a partial
/// public output.
pub fn hash_while_copying_cancellable(
    src: &Path,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, u64),
) -> std::io::Result<(String, u64, crate::file_identity::PrivateFile)> {
    hash_while_copying_detailed(src, dst, cancelled, progress, |_| {})
        .map_err(CopyFailure::into_io)
}

#[derive(Debug)]
pub(crate) enum CopyFailure {
    Cancelled,
    Source(std::io::Error),
    Destination(std::io::Error),
}

impl CopyFailure {
    fn into_io(self) -> std::io::Error {
        match self {
            Self::Cancelled => std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"),
            Self::Source(error) | Self::Destination(error) => error,
        }
    }
}

pub(crate) fn hash_while_copying_cancellable_detailed(
    src: &Path,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(String, u64, crate::file_identity::PrivateFile), CopyFailure> {
    hash_while_copying_detailed(src, dst, cancelled, progress, |_| {})
}

fn hash_while_copying_detailed(
    src: &Path,
    dst: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, u64),
    after_sync: impl FnOnce(&Path),
) -> Result<(String, u64, crate::file_identity::PrivateFile), CopyFailure> {
    // A read or write given up on because Cancel was pressed is a
    // cancellation, not a failure of either side.
    let classify = |error: std::io::Error, side: fn(std::io::Error) -> CopyFailure| {
        if cancelled()
            && (error.kind() == std::io::ErrorKind::Interrupted
                || volume_io::wait_failure(&error).is_some())
        {
            CopyFailure::Cancelled
        } else {
            side(error)
        }
    };
    // The source is the regular file currently at the recorded path: a
    // symlink put there is not followed and a FIFO is refused without waiting.
    let (mut reader, _) = crate::file_identity::open_regular_nofollow(src)
        .map_err(|error| classify(error, CopyFailure::Source))?;
    let expected_total = reader
        .metadata()
        .map_err(|error| classify(error, CopyFailure::Source))?
        .len();
    let writer =
        volume_io::create_new(dst).map_err(|error| classify(error, CopyFailure::Destination))?;
    // From here every failure or cancellation drops the private output, which
    // removes it while `dst` still names this descriptor's file.
    let mut private = crate::file_identity::PrivateFile::new(dst.to_path_buf(), writer);
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; BUF_SIZE];
    let mut total: u64 = 0;
    progress(total, expected_total);

    let writer = private.file_mut();
    loop {
        if cancelled() {
            return Err(CopyFailure::Cancelled);
        }
        let n = reader
            .read_cancellable(&mut buf, Some(cancelled))
            .map_err(|error| classify(error, CopyFailure::Source))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        writer
            .write_all_cancellable(&buf[..n], Some(cancelled))
            .map_err(|error| classify(error, CopyFailure::Destination))?;
        total += n as u64;
        progress(total, expected_total);
    }
    writer
        .sync_all(total, Some(cancelled))
        .map_err(|error| classify(error, CopyFailure::Destination))?;
    after_sync(dst);
    let streamed_hash = hasher.finalize().to_hex().to_string();

    // Verify through the same descriptor that received the bytes. Reopening
    // `dst` would make a pathname replacement the object being verified.
    writer
        .seek(SeekFrom::Start(0))
        .map_err(|error| classify(error, CopyFailure::Destination))?;
    let mut read_back = blake3::Hasher::new();
    loop {
        // The output is still private, so a cancel here abandons it like
        // one during the copy; a large file never pins its holder.
        if cancelled() {
            return Err(CopyFailure::Cancelled);
        }
        let n = writer
            .read_cancellable(&mut buf, Some(cancelled))
            .map_err(|error| classify(error, CopyFailure::Destination))?;
        if n == 0 {
            break;
        }
        read_back.update(&buf[..n]);
    }
    if read_back.finalize().to_hex().as_str() != streamed_hash {
        return Err(CopyFailure::Destination(std::io::Error::other(
            "staged destination read-back did not match the copied bytes",
        )));
    }
    Ok((streamed_hash, total, private))
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: the callback is a private
// exact-boundary seam that keeps descriptor ownership out of the public API.
#[path = "../tests/unit/hashing.rs"]
mod descriptor_tests;

fn copy_exact_into(
    file: &mut VolumeFile,
    hasher: &mut blake3::Hasher,
    mut remaining: u64,
) -> std::io::Result<()> {
    let mut buf = vec![0u8; BUF_SIZE.min(PREHASH_WINDOW as usize)];
    while remaining > 0 {
        let want = buf.len().min(remaining as usize);
        let n = file.read_cancellable(&mut buf[..want], None)?;
        if n == 0 {
            break; // size raced smaller since stat; hash what exists
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(())
}

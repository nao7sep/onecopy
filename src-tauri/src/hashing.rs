//! Content hashing, tiered (the indexing pipeline's design): files with a
//! unique size are never content-read; size collisions read a 64 KB head+tail
//! prehash; size+prehash collisions get a full blake3 hash. Only full-hash
//! equality collapses copies — a destructive claim needs certainty — and
//! same-size-same-prehash-different-hash surfaces as the copies-disagree
//! anomaly upstream.
//!
//! `hash_while_copying` is the move/copy-out primitive: the copy reads the
//! source anyway, so teeing the stream through blake3 verifies the chosen copy
//! against the indexed hash at zero extra I/O.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Head/tail window for the prehash tier (public: the tests and any
/// consumer reasoning about the tier need the exact spec value).
pub const PREHASH_WINDOW: u64 = 64 * 1024;

/// Streaming buffer for full hashing and tee-copying.
const BUF_SIZE: usize = 1024 * 1024;

/// blake3 over the first and last 64 KB (whole file when ≤128 KB). Cheap
/// same-size disambiguation only — never a collapse criterion.
pub fn prehash(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
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
    let file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(file)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// `full_hash` with a cooperative cancel checked between read chunks, so app
/// exit interrupts a multi-gigabyte hash in bounded time. Scan-only — the
/// move-out tee (`hash_while_copying`) deliberately has no cancel: a delivery
/// verify must never be abandoned halfway.
pub fn full_hash_cancellable(
    path: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Copies `src` to `dst` while hashing the bytes read — the tee that gives
/// move/copy-out its free source verification. Returns (hash, bytes copied).
/// not recorded: this writes the user's own media into a destination root —
/// OUTPUT, not app-managed text (data-backup conventions).
/// The destination is created fresh (never clobbering an existing file: the
/// collision policy upstream decides skips/conflicts before this runs) and
/// fsynced before return; renaming/staging discipline belongs to the caller.
pub fn hash_while_copying(src: &Path, dst: &Path) -> std::io::Result<(String, u64)> {
    let mut reader = File::open(src)?;
    let mut writer = File::create_new(dst)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; BUF_SIZE];
    let mut total: u64 = 0;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        writer.write_all(&buf[..n])?;
        total += n as u64;
    }
    writer.sync_all()?;

    Ok((hasher.finalize().to_hex().to_string(), total))
}

fn copy_exact_into(
    file: &mut File,
    hasher: &mut blake3::Hasher,
    mut remaining: u64,
) -> std::io::Result<()> {
    let mut buf = vec![0u8; BUF_SIZE.min(PREHASH_WINDOW as usize)];
    while remaining > 0 {
        let want = buf.len().min(remaining as usize);
        let n = file.read(&mut buf[..want])?;
        if n == 0 {
            break; // size raced smaller since stat; hash what exists
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(())
}

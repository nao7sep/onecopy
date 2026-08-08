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

/// Head/tail window for the prehash tier.
const PREHASH_WINDOW: u64 = 64 * 1024;

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

/// Copies `src` to `dst` while hashing the bytes read — the tee that gives
/// move/copy-out its free source verification. Returns (hash, bytes copied).
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(label: &str, bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix(&format!("onecopy-hash-{label}-"))
            .tempdir()
            .unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn full_hash_matches_one_shot_blake3() {
        let bytes = vec![7u8; 300_000];
        let (_dir, path) = temp_file("full", &bytes);
        assert_eq!(
            full_hash(&path).unwrap(),
            blake3::hash(&bytes).to_hex().to_string()
        );
    }

    #[test]
    fn prehash_of_small_file_hashes_all_bytes_once() {
        // ≤64 KB: the prehash is the hash of the whole content.
        let bytes = b"tiny file".to_vec();
        let (_dir, path) = temp_file("small", &bytes);
        assert_eq!(
            prehash(&path).unwrap(),
            blake3::hash(&bytes).to_hex().to_string()
        );
    }

    #[test]
    fn prehash_between_one_and_two_windows_never_double_counts() {
        // 100 KB: head 64 KB + remaining 36 KB, hashed exactly once each.
        let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let (_dir, path) = temp_file("mid", &bytes);
        assert_eq!(
            prehash(&path).unwrap(),
            blake3::hash(&bytes).to_hex().to_string()
        );
    }

    #[test]
    fn prehash_of_large_file_is_head_plus_tail() {
        let bytes: Vec<u8> = (0..400_000u32).map(|i| (i % 249) as u8).collect();
        let (_dir, path) = temp_file("large", &bytes);
        let window = PREHASH_WINDOW as usize;
        let mut expected = blake3::Hasher::new();
        expected.update(&bytes[..window]);
        expected.update(&bytes[bytes.len() - window..]);
        assert_eq!(
            prehash(&path).unwrap(),
            expected.finalize().to_hex().to_string()
        );
    }

    #[test]
    fn prehash_distinguishes_differing_edges_but_not_middles() {
        // Same size, difference only in the middle: prehash agrees (which is
        // exactly why it never collapses copies) while full hash differs.
        let mut a: Vec<u8> = vec![1; 400_000];
        let mut b: Vec<u8> = vec![1; 400_000];
        a[200_000] = 9;
        b[200_000] = 8;
        let (_da, pa) = temp_file("mid-a", &a);
        let (_db, pb) = temp_file("mid-b", &b);
        assert_eq!(prehash(&pa).unwrap(), prehash(&pb).unwrap());
        assert_ne!(full_hash(&pa).unwrap(), full_hash(&pb).unwrap());

        // A difference within the head window does change the prehash.
        let mut c = a.clone();
        c[10] = 77;
        let (_dc, pc) = temp_file("head-c", &c);
        assert_ne!(prehash(&pa).unwrap(), prehash(&pc).unwrap());
    }

    #[test]
    fn hash_while_copying_copies_exactly_and_hashes_the_stream() {
        let bytes: Vec<u8> = (0..250_000u32).map(|i| (i % 241) as u8).collect();
        let (_dir, src) = temp_file("tee", &bytes);
        let dst_dir = tempfile::Builder::new()
            .prefix("onecopy-hash-tee-dst-")
            .tempdir()
            .unwrap();
        let dst = dst_dir.path().join("out.bin");

        let (hash, total) = hash_while_copying(&src, &dst).unwrap();
        assert_eq!(total, bytes.len() as u64);
        assert_eq!(hash, blake3::hash(&bytes).to_hex().to_string());
        assert_eq!(std::fs::read(&dst).unwrap(), bytes);
    }

    #[test]
    fn hash_while_copying_refuses_to_clobber_an_existing_destination() {
        let (_dir, src) = temp_file("noclobber", b"data");
        let dst_dir = tempfile::Builder::new()
            .prefix("onecopy-hash-noclobber-")
            .tempdir()
            .unwrap();
        let dst = dst_dir.path().join("out.bin");
        std::fs::write(&dst, b"already here").unwrap();
        assert!(hash_while_copying(&src, &dst).is_err());
        // The existing file is untouched.
        assert_eq!(std::fs::read(&dst).unwrap(), b"already here");
    }
}

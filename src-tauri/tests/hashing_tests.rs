// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::hashing::*;

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

    let (hash, total, private) = hash_while_copying(&src, &dst).unwrap();
    assert_eq!(total, bytes.len() as u64);
    assert_eq!(private.path(), dst);
    assert_eq!(hash, blake3::hash(&bytes).to_hex().to_string());
    assert_eq!(std::fs::read(&dst).unwrap(), bytes);
}

#[test]
fn cancellable_copy_removes_its_owned_private_output() {
    let bytes = vec![3u8; 2 * 1024 * 1024];
    let (_dir, src) = temp_file("tee-cancel", &bytes);
    let dst_dir = tempfile::tempdir().unwrap();
    let dst = dst_dir.path().join("private.tmp");
    let stop = std::cell::Cell::new(false);
    let error = hash_while_copying_cancellable(&src, &dst, &|| stop.get(), &mut |done, _| {
        if done > 0 {
            stop.set(true);
        }
    })
    .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(!dst.exists());
}

#[test]
fn cancellable_hash_matches_plain_and_stops_on_cancel() {
    use std::sync::atomic::AtomicBool;

    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 239) as u8).collect();
    let (_dir, path) = temp_file("cancellable", &bytes);

    let calm = AtomicBool::new(false);
    assert_eq!(
        full_hash_with_cancel(&path, &|| calm.load(std::sync::atomic::Ordering::Relaxed)).unwrap(),
        full_hash(&path).unwrap()
    );

    let cancelled = AtomicBool::new(true);
    let err = full_hash_with_cancel(&path, &|| cancelled.load(std::sync::atomic::Ordering::Relaxed)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
}

#[test]
fn streamed_hash_progress_is_descriptor_exact_and_monotonic() {
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    let bytes = vec![5u8; 2_500_000];
    let (_dir, path) = temp_file("progress", &bytes);
    let observed = Mutex::new(Vec::<(u64, u64)>::new());

    full_hash_cancellable_with_progress(&path, &AtomicBool::new(false), &|done, total| {
        observed.lock().unwrap().push((done, total));
    })
    .unwrap();

    let observed = observed.into_inner().unwrap();
    assert_eq!(observed.first(), Some(&(0, bytes.len() as u64)));
    assert_eq!(
        observed.last(),
        Some(&(bytes.len() as u64, bytes.len() as u64))
    );
    assert!(observed.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    assert!(observed
        .iter()
        .all(|(_, total)| *total == bytes.len() as u64));
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

#[cfg(unix)]
#[test]
fn copying_reads_only_the_regular_file_at_the_source_path() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside.jpg");
    std::fs::write(&outside, b"not the recorded file").unwrap();
    let link = dir.path().join("link.jpg");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let dst = dir.path().join("out-link.tmp");
    assert!(hash_while_copying(&link, &dst).is_err());
    assert!(!dst.exists());

    // A FIFO at the source path is refused at once instead of blocking the
    // operation until a writer appears.
    let fifo = dir.path().join("pipe.jpg");
    let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let dst = dir.path().join("out-fifo.tmp");
    assert!(hash_while_copying(&fifo, &dst).is_err());
    assert!(!dst.exists());
}

// Windows has no FIFOs, and a file symlink needs Developer Mode or elevation
// to create, so the junction half always runs and the symlink half runs where
// the machine allows it.
#[cfg(windows)]
#[test]
fn copying_never_reads_through_a_junction_or_file_symlink_at_the_source_path() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("inside.jpg"), b"inside").unwrap();
    let junction = dir.path().join("junction.jpg");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&folder)
        .output()
        .unwrap();
    assert!(made.status.success(), "mklink /J failed: {}", String::from_utf8_lossy(&made.stderr));
    let dst = dir.path().join("out-junction.tmp");
    assert!(hash_while_copying(&junction, &dst).is_err());
    assert!(!dst.exists());

    let outside = dir.path().join("outside.jpg");
    std::fs::write(&outside, b"not the recorded file").unwrap();
    let link = dir.path().join("link.jpg");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap();
    if !made.status.success() {
        eprintln!(
            "skipping the file-symlink half: mklink needs Developer Mode or elevation ({}{})",
            String::from_utf8_lossy(&made.stdout).trim(),
            String::from_utf8_lossy(&made.stderr).trim()
        );
        return;
    }
    let dst = dir.path().join("out-link.tmp");
    assert!(hash_while_copying(&link, &dst).is_err());
    assert!(!dst.exists());
}

#[test]
fn a_dropped_unpublished_copy_removes_its_private_output() {
    let (_dir, src) = temp_file("drop", b"bytes");
    let dst_dir = tempfile::tempdir().unwrap();
    let dst = dst_dir.path().join("private.tmp");
    let (_, _, private) = hash_while_copying(&src, &dst).unwrap();
    assert!(dst.exists());
    drop(private);
    assert!(!dst.exists());
}

// What a copy keeps of its source (content-lifecycle conventions, Files).

fn at(seconds: u64) -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)
}

fn set_source_times(path: &std::path::Path, modified: std::time::SystemTime) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
}

#[test]
fn a_copy_keeps_the_source_modified_time() {
    let (dir, src) = temp_file("keep-modified", b"dated bytes");
    set_source_times(&src, at(1_500_000_000));
    let dst = dir.path().join("out.bin");

    let (_, _, private) = hash_while_copying(&src, &dst).unwrap();

    assert_eq!(std::fs::metadata(&dst).unwrap().modified().unwrap(), at(1_500_000_000));
    drop(private);
}

#[test]
fn a_read_only_source_copies_read_only_and_an_unpublished_copy_is_still_removed() {
    let (dir, src) = temp_file("keep-read-only", b"locked bytes");
    let mut permissions = std::fs::metadata(&src).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&src, permissions).unwrap();
    let dst = dir.path().join("out.bin");

    let (hash, _, private) = hash_while_copying(&src, &dst).unwrap();

    assert_eq!(hash, blake3::hash(b"locked bytes").to_hex().to_string());
    assert!(std::fs::metadata(&dst).unwrap().permissions().readonly());
    drop(private);
    assert!(!dst.exists(), "cleanup removes a read-only private output");
}

#[cfg(unix)]
#[test]
fn a_copy_keeps_the_source_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, src) = temp_file("keep-mode", b"shared bytes");
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o640)).unwrap();
    let dst = dir.path().join("out.bin");

    let (_, _, private) = hash_while_copying(&src, &dst).unwrap();

    assert_eq!(std::fs::metadata(&dst).unwrap().permissions().mode() & 0o7777, 0o640);
    drop(private);
}

windows_test! {
    "Windows only: a failed pinned-handle proof retains the original",
    fn windows_trash_retains_original_when_deletion_handle_proof_fails() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original.bin");
        std::fs::write(&original, b"retain this version").unwrap();
        let error = super::windows::remove_original(&original, |file| {
            assert_eq!(file.metadata()?.len(), 19);
            Err(std::io::Error::other("source version changed"))
        }).unwrap_err();
        assert_eq!(error.to_string(), "source version changed");
        assert_eq!(std::fs::read(&original).unwrap(), b"retain this version");
    }
}

windows_test! {
    "Windows only: read-only trash source removal preserves surviving hard-link attributes",
    fn windows_trash_removes_a_readonly_original_without_mutating_its_live_hardlink() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original.bin");
        let live = root.path().join("live-hardlink.bin");
        std::fs::write(&original, b"recoverable readonly bytes").unwrap();
        std::fs::hard_link(&original, &live).unwrap();
        let mut permissions = std::fs::metadata(&original).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&original, permissions).unwrap();
        let before = security_text(&std::fs::File::open(&live).unwrap());

        super::windows::remove_original(&original, |_| Ok(())).unwrap();

        assert!(!original.exists());
        assert_eq!(std::fs::read(&live).unwrap(), b"recoverable readonly bytes");
        assert!(std::fs::metadata(&live).unwrap().permissions().readonly());
        assert_eq!(security_text(&std::fs::File::open(&live).unwrap()), before);
        // Fixture cleanup only, after proving the surviving link was unchanged.
        let mut permissions = std::fs::metadata(&live).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&live, permissions).unwrap();
    }
}

windows_test! {
    "Windows only: manifest appends validate a protected file without truncation",
    fn windows_trash_manifest_reopens_at_the_end() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private-trash");
        let _guard = super::windows::open_directory(&path, true).unwrap();
        let manifest = path.join("manifest.jsonl");
        {
            let mut file = super::windows::open_manifest(&manifest).unwrap();
            file.write_all(b"first\n").unwrap();
        }
        {
            let mut file = super::windows::open_manifest(&manifest).unwrap();
            file.write_all(b"second\n").unwrap();
        }
        assert_eq!(std::fs::read(manifest).unwrap(), b"first\nsecond\n");
    }
}

windows_test! {
    "Windows only: broad existing manifests are refused without changing bytes or ACLs",
    fn windows_trash_manifest_rejects_a_broad_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private-trash");
        let _guard = super::windows::open_directory(&private, true).unwrap();
        let broad = root.path().join("broad");
        let key = super::windows::account_key().unwrap();
        create_directory_with_security(&broad, &format!("O:{key}D:P(A;OICI;FA;;;{key})(A;OICI;FR;;;WD)"));
        let source = broad.join("manifest.jsonl");
        std::fs::write(&source, b"preserve existing bytes\n").unwrap();
        let manifest = private.join("manifest.jsonl");
        // A same-volume rename preserves the broad source file's ACL.
        std::fs::rename(source, &manifest).unwrap();
        let read = std::fs::File::open(&manifest).unwrap();
        let before = security_text(&read);
        assert!(before.contains("WD"));
        assert_eq!(super::windows::open_manifest(&manifest).unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(security_text(&read), before);
        assert_eq!(std::fs::read(manifest).unwrap(), b"preserve existing bytes\n");
    }
}

windows_test! {
    "Windows only: private trash directories use protected token-owned ACLs",
    fn creates_and_reopens_a_private_windows_trash_directory() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private-trash");
        let directory = super::windows::open_directory(&path, true).unwrap();
        let key = super::windows::account_key().unwrap();
        assert!(key.starts_with("S-"));
        assert!(key.bytes().all(|b| b == b'S' || b == b'-' || b.is_ascii_digit()));
        assert_private_security(&directory, &key, true);
        let reopened = super::windows::open_directory(&path, false).unwrap();
        drop(reopened);
        drop(directory);
    }
}

windows_test! {
    "Windows only: broad existing trash directories are rejected without ACL changes",
    fn rejects_a_broad_existing_windows_trash_directory_without_tightening_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("broad-trash");
        let key = super::windows::account_key().unwrap();
        create_directory_with_security(&path, &format!("O:{key}D:P(A;OICI;FA;;;{key})(A;OICI;FR;;;WD)"));
        let before = directory_security_text(&path);
        let error = super::windows::open_directory(&path, true).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(directory_security_text(&path), before);
    }
}

windows_test! {
    "Windows only: unprotected current-user-only directories are rejected",
    fn rejects_an_unprotected_windows_trash_dacl() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("unprotected-trash");
        let key = super::windows::account_key().unwrap();
        create_directory_with_security(&path, &format!("O:{key}D:(A;OICI;FA;;;{key})"));
        let before = directory_security_text(&path);
        assert_eq!(super::windows::open_directory(&path, false).unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(directory_security_text(&path), before);
    }
}

windows_test! {
    "Windows only: a private trash directory handle pins its pathname",
    fn an_open_windows_trash_directory_prevents_rename_or_removal() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private-trash");
        let renamed = root.path().join("renamed-trash");
        let guard = super::windows::open_directory(&path, true).unwrap();
        assert!(std::fs::rename(&path, &renamed).is_err());
        assert!(std::fs::remove_dir(&path).is_err());
        assert!(path.is_dir());
        drop(guard);
        std::fs::rename(&path, &renamed).unwrap();
        std::fs::remove_dir(&renamed).unwrap();
    }
}

windows_test! {
    "Windows only: new trash files are private and remain publishable while open",
    fn windows_trash_file_creation_protects_the_file_and_allows_publication() {
        use std::io::{Read, Seek, SeekFrom, Write};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private-trash");
        let _guard = super::windows::open_directory(&path, true).unwrap();
        let staged = path.join("staged.tmp");
        let published = path.join("stored.bin");
        let mut file = super::windows::create_file(&staged).unwrap();
        file.write_all(b"private contents").unwrap();
        file.sync_all().unwrap();
        let key = super::windows::account_key().unwrap();
        assert_private_security(&file, &key, false);
        assert_restricted_token_cannot_read(&file);
        assert_eq!(super::windows::create_file(&staged).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        std::fs::rename(&staged, &published).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"private contents");
        assert_eq!(std::fs::read(published).unwrap(), bytes);
    }
}

windows_test! {
    "Windows only: private trash refuses directory reparse points",
    fn windows_trash_does_not_follow_a_directory_symlink() {
        // Junction creation needs no symlink privilege; use the existing fixture
        // helper's command shape rather than requiring Developer Mode.
        use std::os::windows::process::CommandExt;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let _target = super::windows::open_directory(&target, true).unwrap();
        std::fs::write(target.join("preserved.bin"), b"preserved target").unwrap();
        let private = root.path().join("private-trash");
        let _guard = super::windows::open_directory(&private, true).unwrap();
        let junction = private.join("manifest.jsonl");
        let output = std::process::Command::new("cmd")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&junction).arg(&target).creation_flags(0x0800_0000)
            .output().unwrap();
        assert!(output.status.success(), "junction fixture failed: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(super::windows::open_directory(&junction, false).unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        assert!(super::windows::open_manifest(&junction).is_err());
        assert_eq!(super::windows::remove_original(&junction, |_| Ok(())).unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read(target.join("preserved.bin")).unwrap(), b"preserved target");
        std::fs::remove_dir(junction).unwrap();
    }
}

#[cfg(windows)]
#[path = "windows_security_fixture.rs"]
mod security_fixture;
#[cfg(windows)]
use security_fixture::*;

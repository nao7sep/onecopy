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
fn create_directory_with_security(path: &std::path::Path, sddl: &str) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor = std::ptr::null_mut();
    unsafe {
        assert_ne!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut()
            ),
            0
        );
    }
    let descriptor = TestLocalAllocation(descriptor);
    let attributes = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    unsafe {
        assert_ne!(
            windows_sys::Win32::Storage::FileSystem::CreateDirectoryW(path.as_ptr(), &attributes),
            0
        );
    }
}

#[cfg(windows)]
struct TestLocalAllocation(*mut std::ffi::c_void);

#[cfg(windows)]
impl Drop for TestLocalAllocation {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::LocalFree(self.0) };
    }
}

#[cfg(windows)]
fn directory_security_text(path: &std::path::Path) -> String {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, READ_CONTROL,
    };
    let directory = std::fs::OpenOptions::new()
        .access_mode(READ_CONTROL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .unwrap();
    security_text(&directory)
}

#[cfg(windows)]
fn security_descriptor(file: &std::fs::File) -> TestLocalAllocation {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    };
    let mut descriptor = std::ptr::null_mut();
    unsafe {
        assert_eq!(
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor
            ),
            0
        );
    }
    TestLocalAllocation(descriptor)
}

#[cfg(windows)]
fn security_text(file: &std::fs::File) -> String {
    use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION};
    let descriptor = security_descriptor(file);
    let mut text = std::ptr::null_mut();
    unsafe {
        assert_ne!(
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor.0,
                1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut text,
                std::ptr::null_mut()
            ),
            0
        );
    }
    let _allocation = TestLocalAllocation(text.cast());
    let mut length = 0;
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap()
    }
}

#[cfg(windows)]
fn assert_private_security(file: &std::fs::File, key: &str, directory: bool) {
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
    let descriptor = security_descriptor(file);
    let mut owner = std::ptr::null_mut();
    let mut defaulted = 0;
    let mut text = std::ptr::null_mut();
    let mut control = 0;
    let mut revision = 0;
    let mut present = 0;
    let mut acl = std::ptr::null_mut();
    unsafe {
        assert_ne!(
            GetSecurityDescriptorOwner(descriptor.0, &mut owner, &mut defaulted),
            0
        );
        assert_ne!(ConvertSidToStringSidW(owner, &mut text), 0);
        let _text = TestLocalAllocation(text.cast());
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        assert_eq!(
            String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap(),
            key
        );
        assert_ne!(
            GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision),
            0
        );
        // Auto-inheritance bookkeeping may differ across Windows filesystems;
        // protection and the actual single explicit ACE are the privacy contract.
        assert_ne!(control & SE_DACL_PROTECTED, 0);
        assert_ne!(
            GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted),
            0
        );
        assert_ne!(present, 0);
        assert!(!acl.is_null());
        assert_ne!(IsValidAcl(acl), 0);
        assert_eq!((*acl).AceCount, 1);
        let mut ace = std::ptr::null_mut();
        assert_ne!(GetAce(acl, 0, &mut ace), 0);
        let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
        assert_eq!(ace.Header.AceType, ACCESS_ALLOWED_ACE_TYPE as u8);
        let flags = if directory {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            0
        };
        assert_eq!(ace.Header.AceFlags, flags as u8);
        assert_eq!(ace.Mask, FILE_ALL_ACCESS);
        let sid = std::ptr::addr_of!(ace.SidStart).cast_mut().cast();
        assert_ne!(EqualSid(sid, owner), 0);
    }
}

#[cfg(windows)]
fn assert_restricted_token_cannot_read(file: &std::fs::File) {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_READ_DATA,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut process = std::ptr::null_mut();
    unsafe {
        assert_ne!(
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_DUPLICATE | TOKEN_QUERY,
                &mut process
            ),
            0
        );
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    // Restrict access to Everyone as a second independent access check. The
    // actual file's current-user-only DACL must reject that restricted identity.
    let mut world = [0usize; 16];
    let mut world_bytes = std::mem::size_of_val(&world) as u32;
    unsafe {
        assert_ne!(
            CreateWellKnownSid(
                WinWorldSid,
                std::ptr::null_mut(),
                world.as_mut_ptr().cast(),
                &mut world_bytes
            ),
            0
        );
    }
    let restricted_sid = SID_AND_ATTRIBUTES {
        Sid: world.as_mut_ptr().cast(),
        Attributes: 0,
    };
    let mut restricted = std::ptr::null_mut();
    unsafe {
        assert_ne!(
            CreateRestrictedToken(
                process.as_raw_handle(),
                DISABLE_MAX_PRIVILEGE,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                1,
                &restricted_sid,
                &mut restricted
            ),
            0
        );
    }
    let restricted = unsafe { OwnedHandle::from_raw_handle(restricted) };
    let mut client = std::ptr::null_mut();
    unsafe {
        assert_ne!(
            DuplicateToken(
                restricted.as_raw_handle(),
                SecurityImpersonation,
                &mut client
            ),
            0
        );
    }
    let client = unsafe { OwnedHandle::from_raw_handle(client) };
    let descriptor = security_descriptor(file);
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    let mut privileges = [0usize; 32];
    let mut privilege_bytes = std::mem::size_of_val(&privileges) as u32;
    let mut granted = 0;
    let mut allowed = 0;
    unsafe {
        assert_ne!(
            AccessCheck(
                descriptor.0,
                client.as_raw_handle(),
                FILE_READ_DATA,
                &mapping,
                privileges.as_mut_ptr().cast(),
                &mut privilege_bytes,
                &mut granted,
                &mut allowed
            ),
            0
        );
    }
    assert_eq!(
        allowed, 0,
        "restricted identity must not read private trash contents"
    );
}

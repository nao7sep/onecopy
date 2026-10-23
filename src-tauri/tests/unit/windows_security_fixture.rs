// Native security fixtures are shared by privacy tests with different assertions.
#![allow(dead_code)]

#[cfg(windows)]
pub(super) fn create_directory_with_security(path: &std::path::Path, sddl: &str) {
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
pub(super) fn directory_security_text(path: &std::path::Path) -> String {
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
pub(super) fn security_text(file: &std::fs::File) -> String {
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
pub(super) fn assert_private_security(file: &std::fs::File, key: &str, directory: bool) {
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
pub(super) fn assert_restricted_token_cannot_read(file: &std::fs::File) {
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

//! Raw Windows security operations. Trash calls run on their owning volume
//! worker; app-root calls run during root setup or on the recovery worker.

use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    LocalFree, ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN, GENERIC_READ,
    GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetAce, GetSecurityDescriptorControl, GetTokenInformation, IsValidAcl, IsValidSid,
    TokenUser, ACCESS_ALLOWED_ACE, ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
    OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, SE_DACL_PROTECTED,
    TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, CreateFileW, FileDispositionInfoEx, GetFileInformationByHandle,
    GetVolumeInformationByHandleW, SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    CREATE_NEW, DELETE, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    READ_CONTROL,
};
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, FILE_PERSISTENT_ACLS};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
};

/// A LocalAlloc-owned descriptor or SID string returned by the security API.
struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: the security APIs transfer a LocalAlloc allocation to us.
        unsafe { LocalFree(self.0) };
    }
}

struct CurrentUser {
    // TokenUser contains an embedded pointer, so retain its aligned buffer.
    buffer: Vec<usize>,
}

impl CurrentUser {
    fn get() -> io::Result<Self> {
        let mut token = null_mut();
        // Use the effective token if this worker is ever impersonating.
        // SAFETY: pseudo handles need no closing; token is an output pointer.
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NO_TOKEN as i32) {
                return Err(error);
            }
            // SAFETY: token is writable and the process pseudo handle is valid.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        // SAFETY: a successful token open transfers the handle to this owner.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut bytes = 0;
        // SAFETY: the null buffer requests the required size only.
        let result = unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut bytes)
        };
        if result != 0
            || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        // SAFETY: buffer has the requested capacity and native pointer alignment.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { buffer })
    }

    fn sid(&self) -> PSID {
        // SAFETY: a successful TokenUser query initialized this aligned buffer,
        // including a SID pointer into that same retained allocation.
        unsafe { (*(self.buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid }
    }

    fn key(&self) -> io::Result<String> {
        let mut text = null_mut();
        // SAFETY: sid belongs to the retained TokenUser buffer; text is output.
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let allocation = LocalAllocation(text.cast());
        let mut length = 0;
        // SAFETY: successful conversion returns a NUL-terminated SID string.
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: length was measured within the returned string allocation.
        let key = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid token SID text"))?;
        drop(allocation);
        // This value becomes one filename component; accept only SID grammar.
        if !key.starts_with("S-") || !key[2..].bytes().all(|b| b.is_ascii_digit() || b == b'-') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid token SID key",
            ));
        }
        Ok(key)
    }

    fn descriptor(&self, directory: bool) -> io::Result<LocalAllocation> {
        let key = self.key()?;
        let inheritance = if directory { "OICI" } else { "" };
        let sddl: Vec<u16> = format!("O:{key}D:P(A;{inheritance};FA;;;{key})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = null_mut();
        // SAFETY: sddl is NUL terminated and descriptor is an output pointer.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(LocalAllocation(descriptor))
    }
}

pub(crate) fn account_key() -> io::Result<String> {
    CurrentUser::get()?.key()
}

fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private storage requires an absolute path without parent traversal",
        ));
    }
    let mut units: Vec<u16> = crate::winpath::for_fs(path)
        .as_os_str()
        .encode_wide()
        .collect();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains NUL",
        ));
    }
    units.push(0);
    Ok(units)
}

fn attributes(descriptor: &LocalAllocation) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}

pub(crate) fn open_directory(path: &Path, create: bool) -> io::Result<File> {
    let path = wide_path(path)?;
    let user = CurrentUser::get()?;
    if create {
        let descriptor = user.descriptor(true)?;
        let attributes = attributes(&descriptor);
        // The private DACL is present at creation, never installed afterward.
        // SAFETY: path, descriptor and attributes remain alive for this call.
        if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 { // volume_io worker or data root
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
                return Err(error);
            }
        }
    }
    // Omitting SHARE_DELETE pins this name until all guarded operations settle.
    // OPEN_REPARSE_POINT inspects the directory itself rather than its target.
    // SAFETY: path is NUL terminated; null security/template use API defaults.
    let handle = unsafe {
        CreateFileW( // volume_io worker or data root
            path.as_ptr(),
            READ_CONTROL | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    let file = owned_file(handle)?;
    require_kind(&file, true)?;
    require_persistent_acls(&file)?;
    require_private_acl(&file, &user, true)?;
    Ok(file)
}

pub(crate) fn create_file(path: &Path) -> io::Result<File> {
    let native = wide_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file has no parent"))?;
    // Keep the checked parent pinned throughout creation and verification.
    let _parent = open_directory(parent, false)?;
    let user = CurrentUser::get()?;
    let descriptor = user.descriptor(false)?;
    let attributes = attributes(&descriptor);
    // CREATE_NEW never adopts another object's permissions or contents. Sharing
    // deletion allows publication by fs_publish while this descriptor stays open.
    // SAFETY: all borrowed buffers remain valid; CREATE_NEW creates a fresh file.
    let handle = unsafe {
        CreateFileW( // volume_io worker or data root
            native.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    let file = owned_file(handle)?;
    require_kind(&file, false)?;
    require_private_acl(&file, &user, false)?;
    Ok(file)
}

pub(crate) fn open_manifest(path: &Path) -> io::Result<File> {
    let mut file = match create_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let native = wide_path(path)?;
            let parent = path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "manifest has no parent")
            })?;
            let _parent = open_directory(parent, false)?;
            // OPEN_EXISTING never truncates; OPEN_REPARSE_POINT lets validation
            // reject a link before any manifest bytes are read or appended.
            // SAFETY: native is NUL terminated and the handle is owned below.
            let handle = unsafe {
                CreateFileW( // volume_io worker or data root
                    native.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
                    FILE_SHARE_READ,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                    null_mut(),
                )
            };
            let file = owned_file(handle)?;
            require_kind(&file, false)?;
            require_private_acl(&file, &CurrentUser::get()?, false)?;
            file
        }
        Err(error) => return Err(error),
    };
    file.seek(SeekFrom::End(0))?;
    Ok(file)
}

fn owned_file(handle: *mut c_void) -> io::Result<File> {
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: CreateFileW transfers a successful file handle to the caller.
        Ok(unsafe { File::from_raw_handle(handle) })
    }
}

/// Remove only this recovered pathname. Clearing read-only attributes would
/// mutate every surviving hard link, so unsupported disposition semantics fail
/// without an attribute-changing fallback.
pub(crate) fn remove_original(path: &Path, verify: impl FnOnce(&File) -> io::Result<()>) -> io::Result<()> {
    let native = wide_path(path)?;
    // Omitting SHARE_DELETE pins the opened name until disposition is set.
    // SAFETY: native is NUL terminated and owned_file adopts a successful handle.
    let handle = unsafe {
        CreateFileW( // volume_io worker or data root
            native.as_ptr(),
            DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    let file = owned_file(handle)?;
    require_kind(&file, false)?;
    verify(&file)?;
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    // SAFETY: file owns a live DELETE-capable handle; disposition has the exact
    // layout and size required by FileDispositionInfoEx. The operation acts on
    // the checked object and never changes its attributes or security descriptor.
    if unsafe {
        SetFileInformationByHandle( // volume_io worker or data root
            file.as_raw_handle(),
            FileDispositionInfoEx,
            std::ptr::from_ref(&disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn require_kind(file: &File, directory: bool) -> io::Result<()> {
    // SAFETY: all-zero is a valid output initialization for this API struct.
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    // SAFETY: file owns a live handle and information is writable.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let flags = information.dwFileAttributes;
    if flags & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (flags & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private storage object has an unsafe file type",
        ));
    }
    Ok(())
}

fn require_persistent_acls(file: &File) -> io::Result<()> {
    let mut flags = 0;
    // Query the opened object's actual volume, including remote volumes.
    // SAFETY: the handle is live; optional outputs are null and flags is writable.
    if unsafe {
        GetVolumeInformationByHandleW( // volume_io worker or data root
            file.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            &mut flags,
            null_mut(),
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if flags & FILE_PERSISTENT_ACLS == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "private storage requires persistent filesystem ACLs",
        ));
    }
    Ok(())
}

fn require_private_acl(file: &File, user: &CurrentUser, directory: bool) -> io::Result<()> {
    let mut owner = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: all outputs are writable; the descriptor owns returned SID/ACL views.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _descriptor = LocalAllocation(descriptor);
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: GetSecurityInfo returned this valid descriptor.
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let denied = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private storage requires a protected current-user-only ACL",
        )
    };
    // A null DACL grants everyone access. Reject it and all unexpected ACEs;
    // existing directories are never repaired or tightened by opening them.
    // SAFETY: non-null owner/ACL pointers are views into the returned descriptor.
    if control & SE_DACL_PROTECTED == 0
        || owner.is_null()
        || dacl.is_null()
        || unsafe { IsValidSid(owner) } == 0
        || unsafe { EqualSid(owner, user.sid()) } == 0
        || unsafe { IsValidAcl(dacl) } == 0
        || unsafe { (*dacl).AceCount } != 1
    {
        return Err(denied());
    }
    let mut ace = null_mut();
    // SAFETY: dacl is valid and has exactly one ACE.
    if unsafe { GetAce(dacl, 0, &mut ace) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a valid ACL contains an ACE header at this address.
    let header = unsafe { &*(ace.cast::<windows_sys::Win32::Security::ACE_HEADER>()) };
    let expected_flags = if directory {
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
    } else {
        0
    };
    if header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8
        || header.AceFlags != expected_flags as u8
        || (header.AceSize as usize) < size_of::<ACCESS_ALLOWED_ACE>()
    {
        return Err(denied());
    }
    // SAFETY: the validated ACE type and size contain the access mask and SID.
    let allowed = unsafe { &*(ace.cast::<ACCESS_ALLOWED_ACE>()) };
    let sid = std::ptr::addr_of!(allowed.SidStart).cast_mut().cast();
    // SAFETY: IsValidAcl validated the ACE's SID storage; SID remains alive.
    if allowed.Mask != FILE_ALL_ACCESS
        || unsafe { IsValidSid(sid) } == 0
        || unsafe { EqualSid(sid, user.sid()) } == 0
    {
        return Err(denied());
    }
    Ok(())
}

/// Creates a private directory without adopting or changing an existing one.
pub(crate) fn create_directory(path: &Path) -> io::Result<bool> {
    let path = wide_path(path)?;
    let descriptor = CurrentUser::get()?.descriptor(true)?;
    let attributes = attributes(&descriptor);
    // SAFETY: all pointed-to buffers live through this call.
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } != 0 { // data root
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) { Ok(false) } else { Err(error) }
}

/// Pins an app-owned object without following links or taking ownership.
/// MAXIMUM_ALLOWED is intentional: SetSecurityInfo then does not propagate
/// changes to child objects. Migration visits only the app's own children.
pub(crate) fn pin_owned(path: &Path, directory: bool) -> io::Result<File> {
    use windows_sys::Win32::System::SystemServices::MAXIMUM_ALLOWED;
    let native = wide_path(path)?;
    // SAFETY: native is NUL terminated and a successful handle is adopted below.
    let handle = unsafe {
        CreateFileW( // data root
            native.as_ptr(), MAXIMUM_ALLOWED, FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    let file = owned_file(handle)?;
    require_kind(&file, directory)?;
    require_persistent_acls(&file)?;
    let mut owner = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: outputs are writable; descriptor retains the returned owner SID.
    let status = unsafe { GetSecurityInfo(
        file.as_raw_handle(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION,
        &mut owner, null_mut(), null_mut(), null_mut(), &mut descriptor,
    ) };
    if status != 0 { return Err(io::Error::from_raw_os_error(status as i32)); }
    let _descriptor = LocalAllocation(descriptor);
    let user = CurrentUser::get()?;
    // SAFETY: a non-null owner is retained by descriptor.
    if owner.is_null() || unsafe { IsValidSid(owner) } == 0 || unsafe { EqualSid(owner, user.sid()) } == 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "data object belongs to another account"));
    }
    if !directory {
        // A DACL belongs to the file, not a link. Changing a multiply linked
        // object could change access to an original outside the app folder.
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
        // SAFETY: file is live and information is a writable output struct.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if information.nNumberOfLinks != 1 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "data object has multiple hard links"));
        }
    }
    Ok(file)
}

pub(crate) fn require_private(file: &File, directory: bool) -> io::Result<()> {
    require_private_acl(file, &CurrentUser::get()?, directory)
}

/// Only accepts a handle opened by pin_owned, with MAXIMUM_ALLOWED. Inherited
/// ACL propagation is suppressed; the caller explicitly owns each mutation.
pub(crate) fn make_private(file: &File, directory: bool) -> io::Result<()> {
    use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION};
    use windows_sys::Win32::Security::Authorization::SetSecurityInfo;
    let user = CurrentUser::get()?;
    let descriptor = user.descriptor(directory)?;
    let mut dacl = null_mut();
    let mut present = 0;
    let mut defaulted = 0;
    // SAFETY: descriptor is valid and remains alive through SetSecurityInfo.
    if unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the pinned handle and descriptor's DACL stay valid for the call.
    let status = unsafe { SetSecurityInfo(
        file.as_raw_handle(), SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
        null_mut(), null_mut(), dacl, null_mut(),
    ) };
    if status != 0 { return Err(io::Error::from_raw_os_error(status as i32)); }
    require_private_acl(file, &user, directory)
}

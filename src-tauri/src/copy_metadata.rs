//! What a copy keeps of its source file: the Files section of the
//! content-lifecycle conventions. `hash_while_copying` reads it from the
//! source descriptor and writes it onto the private output before that output
//! is flushed and published, so every Copy and Move output carries it.
//!
//! Both halves run inside `VolumeFile::with`, on the `volume_io` worker that
//! holds the descriptor. Only the modified time is required: anything else a
//! destination cannot hold is dropped without a word.

use std::fs::{File, Permissions};
use std::io;
use std::time::SystemTime;

pub(crate) struct SourceMetadata {
    modified: SystemTime,
    created: Option<SystemTime>,
    permissions: Permissions,
    #[cfg(target_os = "macos")]
    extended_attributes: Vec<xattr::Attribute>,
    #[cfg(target_os = "macos")]
    acl: Option<Vec<u8>>,
}

impl SourceMetadata {
    pub(crate) fn read(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            modified: metadata.modified()?,
            created: metadata.created().ok(),
            permissions: metadata.permissions(),
            #[cfg(target_os = "macos")]
            extended_attributes: xattr::read_all(file),
            #[cfg(target_os = "macos")]
            acl: acl::read(file),
        })
    }

    /// Fails only when the modified time cannot be set.
    pub(crate) fn apply(&self, file: &File) -> io::Result<()> {
        // Attributes first; access metadata last, so a retained ACL denying
        // writeattr does not prevent the required modified time from landing.
        #[cfg(target_os = "macos")]
        {
            xattr::write_all(file, &self.extended_attributes);
        }
        file.set_times(std::fs::FileTimes::new().set_modified(self.modified))?;
        // Separately, and after the modified time: a volume that refuses a
        // birth time still keeps the modified time, and macOS moves a birth
        // time later than a newly set modified time back to it.
        if let Some(created) = self.created {
            set_created(file, created);
        }
        apply_permissions(file, &self.permissions);
        #[cfg(target_os = "macos")]
        acl::write(file, self.acl.as_deref());
        Ok(())
    }
}

/// Strip inherited native grants from an exclusively created empty stage
/// before any protected bytes are written. Its final access metadata lands later.
pub(crate) fn make_private(file: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    return acl::clear(file);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = file;
        Ok(())
    }
}

pub(crate) fn apply_replacement(source: &File, replacement: &File) -> io::Result<()> {
    apply_permissions(replacement, &source.metadata()?.permissions()); // data root
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        xattr::write_all(replacement, &xattr::read_all(source));
        unsafe {
            libc::fcopyfile(source.as_raw_fd(), replacement.as_raw_fd(), std::ptr::null_mut(), libc::COPYFILE_ACL);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn apply_permissions(file: &File, permissions: &Permissions) {
    let _ = file.set_permissions(permissions.clone());
}

/// Windows permissions are inherited from the destination folder; what a copy
/// carries is the read-only attribute.
#[cfg(windows)]
fn apply_permissions(file: &File, permissions: &Permissions) {
    if !permissions.readonly() {
        return;
    }
    if let Ok(metadata) = file.metadata() {
        let mut readonly = metadata.permissions();
        readonly.set_readonly(true);
        let _ = file.set_permissions(readonly);
    }
}

#[cfg(target_os = "macos")]
fn set_created(file: &File, created: SystemTime) {
    use std::os::macos::fs::FileTimesExt;
    let _ = file.set_times(std::fs::FileTimes::new().set_created(created));
}

#[cfg(windows)]
fn set_created(file: &File, created: SystemTime) {
    use std::os::windows::fs::FileTimesExt;
    let _ = file.set_times(std::fs::FileTimes::new().set_created(created));
}

#[cfg(not(any(target_os = "macos", windows)))]
fn set_created(_file: &File, _created: SystemTime) {}

/// Extended attributes, Finder tags among them. A volume without native
/// extended attributes (exFAT, FAT, some network shares) would store them in
/// `._` files beside the output, so none are written there.
#[cfg(target_os = "macos")]
mod xattr {
    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::AsRawFd;

    pub(super) struct Attribute {
        name: CString,
        value: Vec<u8>,
    }

    /// The source's attributes; one that cannot be read is not kept.
    pub(super) fn read_all(file: &File) -> Vec<Attribute> {
        let fd = file.as_raw_fd();
        let Some(names) = read_sized(|buffer, size| unsafe {
            libc::flistxattr(fd, buffer.cast(), size, 0)
        }) else {
            return Vec::new();
        };
        names
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .filter_map(|name| {
                let name = CString::new(name).ok()?;
                let value = read_sized(|buffer, size| unsafe {
                    libc::fgetxattr(fd, name.as_ptr(), buffer.cast(), size, 0, 0)
                })?;
                Some(Attribute { name, value })
            })
            .collect()
    }

    pub(super) fn write_all(file: &File, attributes: &[Attribute]) {
        let fd = file.as_raw_fd();
        if attributes.is_empty() || !holds_native_attributes(fd) {
            return;
        }
        for attribute in attributes {
            // One the destination refuses (a system-owned name, a size it
            // cannot hold) is dropped; the rest still land.
            unsafe {
                libc::fsetxattr(
                    fd,
                    attribute.name.as_ptr(),
                    attribute.value.as_ptr().cast(),
                    attribute.value.len(),
                    0,
                    0,
                );
            }
        }
    }

    /// Calls `read` once for the size and once for the bytes, again if the
    /// value grew in between.
    fn read_sized(read: impl Fn(*mut u8, usize) -> isize) -> Option<Vec<u8>> {
        for _ in 0..3 {
            let size = read(std::ptr::null_mut(), 0);
            if size <= 0 {
                return (size == 0).then(Vec::new);
            }
            let mut buffer = vec![0u8; size as usize];
            let read_size = read(buffer.as_mut_ptr(), buffer.len());
            if read_size >= 0 {
                buffer.truncate(read_size as usize);
                return Some(buffer);
            }
            if std::io::Error::last_os_error().raw_os_error() != Some(libc::ERANGE) {
                return None;
            }
        }
        None
    }

    /// Whether the volume holding `fd` stores extended attributes itself.
    fn holds_native_attributes(fd: libc::c_int) -> bool {
        #[repr(C, packed(4))]
        struct Reply {
            length: u32,
            capabilities: libc::vol_capabilities_attr_t,
        }
        let mut request: libc::attrlist = unsafe { std::mem::zeroed() };
        request.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
        request.volattr = libc::ATTR_VOL_INFO | libc::ATTR_VOL_CAPABILITIES;
        let mut reply: Reply = unsafe { std::mem::zeroed() };
        let status = unsafe {
            libc::fgetattrlist(
                fd,
                (&mut request as *mut libc::attrlist).cast(),
                (&mut reply as *mut Reply).cast(),
                std::mem::size_of::<Reply>(),
                0,
            )
        };
        if status != 0 {
            return false;
        }
        let capabilities = reply.capabilities;
        let interfaces = libc::VOL_CAPABILITIES_INTERFACES;
        capabilities.capabilities[interfaces]
            & capabilities.valid[interfaces]
            & libc::VOL_CAP_INT_EXTENDED_ATTR
            != 0
    }
}

/// The descriptor's native ACL, serialized so the two volume workers need not
/// share a native allocation. Unsupported ACLs remain optional metadata.
#[cfg(target_os = "macos")]
mod acl {
    use std::{fs::File, os::fd::AsRawFd};
    type Acl = *mut libc::c_void;
    unsafe extern "C" {
        fn acl_init(count: libc::c_int) -> Acl;
        fn acl_get_fd(fd: libc::c_int) -> Acl;
        fn acl_size(acl: Acl) -> libc::ssize_t;
        fn acl_copy_ext(buffer: *mut libc::c_void, acl: Acl, size: libc::ssize_t) -> libc::ssize_t;
        fn acl_copy_int(buffer: *const libc::c_void) -> Acl;
        fn acl_set_fd(fd: libc::c_int, acl: Acl) -> libc::c_int;
        fn acl_free(acl: Acl) -> libc::c_int;
    }
    pub(super) fn clear(file: &File) -> std::io::Result<()> {
        unsafe {
            let acl = acl_init(0);
            if acl.is_null() { return Err(std::io::Error::last_os_error()); }
            let status = acl_set_fd(file.as_raw_fd(), acl);
            let error = (status != 0).then(std::io::Error::last_os_error);
            acl_free(acl);
            match error {
                // A volume without native ACLs cannot have inherited grants.
                Some(error) if matches!(error.raw_os_error(), Some(libc::ENOTSUP) | Some(libc::EOPNOTSUPP)) => Ok(()),
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }
    pub(super) fn read(file: &File) -> Option<Vec<u8>> {
        unsafe {
            let acl = acl_get_fd(file.as_raw_fd());
            if acl.is_null() { return None; }
            let size = acl_size(acl);
            let result = if size > 0 {
                let mut bytes = vec![0; size as usize];
                (acl_copy_ext(bytes.as_mut_ptr().cast(), acl, size) >= 0).then_some(bytes)
            } else { None };
            acl_free(acl);
            result
        }
    }
    pub(super) fn write(file: &File, bytes: Option<&[u8]>) {
        let Some(bytes) = bytes else { return; };
        unsafe {
            let acl = acl_copy_int(bytes.as_ptr().cast());
            if !acl.is_null() {
                acl_set_fd(file.as_raw_fd(), acl);
                acl_free(acl);
            }
        }
    }
}

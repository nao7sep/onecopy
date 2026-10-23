use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        },
    },
    path::Path,
};

pub(super) fn account_key() -> io::Result<String> {
    Ok(format!("uid-{}", unsafe { libc::geteuid() }))
}

pub(super) fn open_directory(path: &Path, create: bool) -> io::Result<File> {
    let created = if create {
        match std::fs::DirBuilder::new().mode(0o700).create(path) { // volume_io worker
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(error),
        }
    } else {
        false
    };
    let file = OpenOptions::new() // volume_io worker
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o777 != 0o700 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "deleted-file directory is not account-private; existing permissions were left unchanged"));
    }
    verify_acl(&file, created)?;
    Ok(file)
}

#[cfg(target_os = "macos")]
fn verify_acl(file: &File, created: bool) -> io::Result<()> {
    // macOS mode bits alone do not exclude inherited extended ACL grants.
    // Only a newly created empty directory may have those grants removed.
    unsafe extern "C" {
        fn acl_init(count: libc::c_int) -> *mut libc::c_void;
        fn acl_get_fd(fd: libc::c_int) -> *mut libc::c_void;
        fn acl_set_fd(fd: libc::c_int, acl: *mut libc::c_void) -> libc::c_int;
        fn acl_get_entry(
            acl: *mut libc::c_void,
            entry_id: libc::c_int,
            entry: *mut *mut libc::c_void,
        ) -> libc::c_int;
        fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
    }
    unsafe {
        let mut status: libc::statfs = std::mem::zeroed();
        if libc::fstatfs(file.as_raw_fd(), &mut status) != 0 { // volume_io worker
            return Err(io::Error::last_os_error());
        }
        if status.f_flags & libc::MNT_IGNORE_OWNERSHIP as u32 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "this volume does not enforce account ownership",
            ));
        }
        if created {
            let empty = acl_init(0);
            if empty.is_null() {
                return Err(io::Error::last_os_error());
            }
            let result = acl_set_fd(file.as_raw_fd(), empty); // volume_io worker
            let error = io::Error::last_os_error();
            acl_free(empty);
            if result != 0 {
                return Err(error);
            }
        }
        let acl = acl_get_fd(file.as_raw_fd()); // volume_io worker
        if acl.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ENOENT) {
                Ok(())
            } else {
                Err(error)
            };
        }
        let mut entry = std::ptr::null_mut();
        // Darwin ACL_FIRST_ENTRY is zero. No ACL entries means no extended
        // grant can bypass the verified owner-only POSIX mode.
        let result = acl_get_entry(acl, 0, &mut entry);
        let error = io::Error::last_os_error();
        acl_free(acl);
        if result == 0 {
            // Darwin returns zero when an entry was found.
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "deleted-file directory has extended access grants",
            ));
        }
        if error.raw_os_error() != Some(libc::EINVAL) {
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn verify_acl(_file: &File, _created: bool) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private deletion is not qualified on this platform",
    ))
}

fn open_at(directory: &File, path: &Path, flags: i32) -> io::Result<File> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("missing stored filename"))?;
    let name =
        CString::new(name.as_bytes()).map_err(|_| io::Error::other("invalid stored filename"))?;
    let fd = unsafe {
        libc::openat( // volume_io worker
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

pub(super) fn open_manifest(directory: &File, path: &Path) -> io::Result<File> {
    let file = open_at(
        directory,
        path,
        libc::O_RDWR | libc::O_APPEND | libc::O_CREAT | libc::O_NONBLOCK,
    )?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("manifest is not a regular file"));
    }
    Ok(file)
}

#[cfg(test)]
pub(super) fn create_file(directory: &File, path: &Path) -> io::Result<File> {
    open_at(directory, path, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL)
}

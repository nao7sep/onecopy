//! Physical file identity for app-created private files, published outputs,
//! and filesystem alias checks.
//!
//! A pathname is only a lookup. Once this operation creates or opens a file,
//! private cleanup and verified publication bind to the filesystem identity
//! returned by that handle so an unrelated replacement is never treated as an
//! app-created temporary output.

#[cfg(not(windows))]
use std::fs::Metadata;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u32,
    #[cfg(windows)]
    index: u64,
}

impl FileIdentity {
    pub fn from_file(file: &File) -> io::Result<Self> {
        #[cfg(windows)]
        {
            return from_windows_file(file);
        }
        #[cfg(not(windows))]
        {
            Self::from_metadata(&file.metadata()?)
        }
    }

    pub fn from_path(path: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            let fs_path = crate::winpath::for_fs(path);
            let mut options = OpenOptions::new();
            options.read(true).custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT
                    | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS,
            );
            return Self::from_file(&options.open(fs_path.as_ref())?);
        }
        #[cfg(not(windows))]
        {
            // Do not follow a replacement symlink: the directory entry itself is
            // not the regular file this operation created.
            Self::from_metadata(&std::fs::symlink_metadata(path)?)
        }
    }

    #[cfg(not(windows))]
    fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = metadata;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "physical file identity is unsupported on this platform",
            ))
        }
    }
}

/// The physical volume holding `path`, following a final symlink. Two paths
/// are on the same filesystem exactly when these agree, however each is
/// spelled (a mapped or `subst` drive, a symlinked root, a verbatim path).
pub fn volume_of(path: &Path) -> io::Result<u64> {
    let fs_path = crate::winpath::for_fs(path);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .access_mode(0)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
            .open(fs_path.as_ref())?;
        return Ok(u64::from(from_windows_file(&file)?.volume));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::metadata(fs_path.as_ref())?.dev())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fs_path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "volume identity is unsupported on this platform",
        ))
    }
}

#[cfg(windows)]
fn from_windows_file(file: &File) -> io::Result<FileIdentity> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: `file` owns a valid handle for this call, and `information` points
    // to writable storage for exactly the structure Windows initializes.
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a nonzero result guarantees the structure was initialized.
    let information = unsafe { information.assume_init() };
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(FileIdentity {
        volume: information.dwVolumeSerialNumber,
        index,
    })
}

/// Opens one existing regular file without following a final symlink/reparse
/// point, then captures the physical identity of that exact descriptor. The
/// open never waits on a special file: a FIFO put at the path is refused
/// rather than blocking the operation that asked.
pub fn open_regular_nofollow(path: &Path) -> io::Result<(File, FileIdentity)> {
    let fs_path = crate::winpath::for_fs(path);
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK only changes opening a FIFO or device; reads of the
        // regular file this function accepts are unaffected by it.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(fs_path.as_ref())?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a no-follow regular file: {}", path.display()),
        ));
    }
    let identity = FileIdentity::from_file(&file)?;
    if !path_names(path, identity) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("file was replaced while opening: {}", path.display()),
        ));
    }
    Ok((file, identity))
}

pub fn path_names(path: &Path, expected: FileIdentity) -> bool {
    FileIdentity::from_path(path).is_ok_and(|actual| actual == expected)
}

/// Whether `path` currently names the file behind `file`. The identity is read
/// from the open descriptor at the moment of the check, not remembered: FAT
/// and exFAT derive an empty file's number from its directory entry, so a
/// rename changes it, while the open descriptor follows the file.
pub fn path_names_file(path: &Path, file: &File) -> bool {
    FileIdentity::from_file(file).is_ok_and(|expected| path_names(path, expected))
}

/// Shared prefix and suffix of every private pathname this app creates for
/// its own bookkeeping beside a real output: a staged output
/// (`.onecopy-stage-<owner>-<nanoid>.tmp`, `private_stage_file_name`) or a
/// publication ownership hold (`.onecopy-claim-<owner>-<nanoid>.tmp`,
/// `claim_private` below). Nothing else ever names a file this way.
const PRIVATE_TMP_PREFIX: &str = ".onecopy-";
const PRIVATE_TMP_SUFFIX: &str = ".tmp";
const STAGE_PREFIX: &str = ".onecopy-stage-";
const CLAIM_PREFIX: &str = ".onecopy-claim-";

/// Fixed widths of the two owner fields embedded right after `STAGE_PREFIX`
/// or `CLAIM_PREFIX`: a fingerprint of this application home, then this
/// process's id, each followed by a `-`. Fixed widths let parsing find the
/// trailing nanoid without guessing where it starts (`parse_private_tmp_owner`).
const HOME_FINGERPRINT_HEX_LEN: usize = 16;
const PID_DECIMAL_LEN: usize = 10; // u32::MAX is 10 decimal digits.

/// File under the data root holding this application home's installation id:
/// a random value generated once, when the root is first used, and never
/// regenerated afterward except when the file is missing. Not recorded: it is
/// a re-derivable-on-loss identity fact, not managed text a user edits or
/// needs restored (`storage.rs`'s write-site table).
pub const INSTALLATION_ID_FILE_NAME: &str = "installation-id";

/// Reads this application home's installation id from `root`, creating it
/// (a fresh nanoid, written unrecorded) the first time this home is used.
fn installation_id(root: &Path) -> io::Result<String> {
    let path = root.join(INSTALLATION_ID_FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let id = contents.trim();
            if id.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "installation id file is empty",
                ));
            }
            Ok(id.to_string())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let id = crate::nanoid::generate().map_err(io::Error::other)?;
            crate::storage::write_atomic_unrecorded(&path, id.as_bytes())
                .map_err(io::Error::other)?;
            Ok(id)
        }
        Err(error) => Err(error),
    }
}

/// This computer's host name, best-effort: empty (never an error) when it
/// cannot be read, since it only narrows a fingerprint that already carries a
/// per-installation random id.
#[cfg(unix)]
fn host_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: `buffer` is valid writable memory of the given length for the
    // whole call.
    let result =
        unsafe { libc::gethostname(buffer.as_mut_ptr() as *mut libc::c_char, buffer.len()) };
    if result != 0 {
        return String::new();
    }
    let len = buffer.iter().position(|&byte| byte == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..len]).into_owned()
}

#[cfg(windows)]
fn host_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

#[cfg(not(any(unix, windows)))]
fn host_name() -> String {
    String::new()
}

/// A 16-hex-character digest combining an application home's installation id
/// with a short hash of this computer's host name.
fn fingerprint_for(installation_id: &str, host_name: &str) -> String {
    let hash = blake3::hash(format!("{installation_id}\0{host_name}").as_bytes());
    hex::encode(&hash.as_bytes()[..HOME_FINGERPRINT_HEX_LEN / 2])
}

/// This process's application-home identity fingerprint, `None` until the
/// data root settles. It carries no meaning beyond telling "this application
/// home, on this computer" apart from every other one, including a different
/// application home that happens to be configured to the exact same path on a
/// different computer (`file-operations.md`, "Recoverable storage and manual
/// recovery"): it combines this home's installation id (`installation_id`, a
/// random value generated once per data root and stored inside it, not the
/// data-root *path*) with a short hash of the host name, so a home is never
/// confused with another home, another computer's home at the same path, or
/// an application home reached over a different network share.
///
/// Before the data root settles (or where none exists, such as most unit
/// tests) this is `None`, on purpose: with no proven identity yet, this
/// process must never claim to own — or match — any private name
/// (`is_abandoned_leftover`), unlike the old fixed placeholder fingerprint
/// that let every unsettled process match every other one.
fn this_application_home_fingerprint() -> Option<&'static str> {
    static FINGERPRINT: OnceLock<Option<String>> = OnceLock::new();
    FINGERPRINT
        .get_or_init(|| {
            let root = crate::paths::data_root().ok()?;
            let installation_id = installation_id(&root).ok()?;
            Some(fingerprint_for(&installation_id, &host_name()))
        })
        .as_ref()
        .map(String::as_str)
}

/// The `<home>-<pid>` owner tag this process embeds in every private staging
/// or claim name it creates. Before the data root settles there is no proven
/// fingerprint yet (`this_application_home_fingerprint`), so this uses a
/// fixed placeholder purely to keep the name well-formed; it is never treated
/// as a match by `is_abandoned_leftover`, which refuses to sweep anything
/// while this process itself is unsettled.
fn this_process_owner_tag() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| {
        let fingerprint = this_application_home_fingerprint()
            .map(str::to_string)
            .unwrap_or_else(|| "0".repeat(HOME_FINGERPRINT_HEX_LEN));
        format!(
            "{fingerprint}-{:0width$}",
            std::process::id(),
            width = PID_DECIMAL_LEN
        )
    })
}

/// A private staging file name beside a real output
/// (`operations::output_stage_path`), embedding this process's owner tag so a
/// later sweep can prove who to remove it from (`is_abandoned_leftover`).
pub fn private_stage_file_name() -> Result<String, String> {
    Ok(format!(
        "{STAGE_PREFIX}{}-{}{PRIVATE_TMP_SUFFIX}",
        this_process_owner_tag(),
        crate::nanoid::generate()?
    ))
}

/// Whether `path`'s file name is one of this app's own private staging or
/// claim pathnames, recognizable purely from its shape, on any application
/// home. A survivor found on a later launch can only be launch-time garbage
/// left by a previous process that quitting gave up on
/// (`app_lifecycle::MUTATION_QUIESCE_DEADLINE`): this process's own mutation
/// admission serializes every file operation against discovery
/// (`scan_runtime::begin_admitted_mutation`), so no live process of this
/// application home ever has one of these names at rest while discovery can
/// see it. Discovery excludes it so it is never indexed as library content —
/// unconditionally, because a live process of a *different* application home
/// may still own it (two homes can share a configured root). Removing it is a
/// separate, ownership-proven decision; see `is_abandoned_leftover`.
pub fn is_private_tmp_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(PRIVATE_TMP_PREFIX) && name.ends_with(PRIVATE_TMP_SUFFIX)
        })
}

struct PrivateTmpOwner {
    home_fingerprint: String,
    pid: u32,
}

/// Parses the owner fields `private_stage_file_name`/`claim_private` embed
/// out of a file name already known to satisfy `is_private_tmp_name`. A name
/// that does not carry them in the expected shape (an older build's leftover,
/// or a name only coincidentally shaped like ours) parses to `None` rather
/// than a guessed owner.
fn parse_private_tmp_owner(name: &str) -> Option<PrivateTmpOwner> {
    let body = name
        .strip_prefix(STAGE_PREFIX)
        .or_else(|| name.strip_prefix(CLAIM_PREFIX))?
        .strip_suffix(PRIVATE_TMP_SUFFIX)?;
    if body.len() < HOME_FINGERPRINT_HEX_LEN + 1 + PID_DECIMAL_LEN + 1 {
        return None;
    }
    let (home_fingerprint, rest) = body.split_at(HOME_FINGERPRINT_HEX_LEN);
    if !home_fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let rest = rest.strip_prefix('-')?;
    let (pid_field, rest) = rest.split_at(PID_DECIMAL_LEN);
    rest.strip_prefix('-')?; // The nanoid follows; its exact contents are not checked.
    let pid: u32 = pid_field.parse().ok()?;
    Some(PrivateTmpOwner {
        home_fingerprint: home_fingerprint.to_string(),
        pid,
    })
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    // Signal 0 sends nothing; it only probes whether the process exists and
    // is visible to this user.
    // SAFETY: `kill` with signal 0 performs no action beyond the existence
    // check itself.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    // EPERM: the process exists but is owned by someone else — still
    // running. Anything else, ESRCH most commonly, means it is gone.
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, STILL_ACTIVE,
    };
    // SAFETY: `OpenProcess` is called with a plain process id and no handle
    // inheritance; a null result is checked before any other call is made,
    // and the handle is always closed once its exit code has been read.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        let read = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        read != 0 && exit_code == STILL_ACTIVE as u32
    }
}

#[cfg(not(any(unix, windows)))]
fn process_is_running(_pid: u32) -> bool {
    // No platform-native liveness probe on this target: never claim a
    // process is dead, so a leftover here is simply never swept rather than
    // risking a live file.
    true
}

/// Whether `path` is this application home's own private staging/claim
/// leftover, proven abandoned: its name embeds this home's fingerprint, and
/// no process with the embedded process id is currently running. A name from
/// a different application home, or one that does not parse as an owner tag
/// at all, is never removed by this check — ownership must be proven, not
/// assumed, so a live process's file (this home's current operation, or a
/// different home's in-progress one sharing this folder) is always left
/// alone. There is deliberately no age-based fallback: unlike a bounded
/// managed-dependency download, a file operation has no whole-operation
/// deadline, so a large in-progress copy staying open for a long time must
/// never be mistaken for abandoned. Likewise, while this process's own
/// fingerprint is not yet settled it proves nothing, so it sweeps nothing.
pub fn is_abandoned_leftover(path: &Path) -> bool {
    is_abandoned_leftover_against(path, this_application_home_fingerprint())
}

/// The pure decision behind `is_abandoned_leftover`, taking the current
/// process's fingerprint explicitly so it can be exercised for every case
/// (unsettled, this home, a different home) without depending on process
/// globals. `current_fingerprint` is `None` exactly when this process itself
/// has not settled its own identity yet, in which case nothing is ever
/// abandoned as far as it can prove.
fn is_abandoned_leftover_against(path: &Path, current_fingerprint: Option<&str>) -> bool {
    let Some(current_fingerprint) = current_fingerprint else {
        return false;
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(owner) = parse_private_tmp_owner(name) else {
        return false;
    };
    owner.home_fingerprint == current_fingerprint && !process_is_running(owner.pid)
}

/// Removes this application home's abandoned private staging/claim leftovers
/// sitting directly in `dir`, before an operation writes its own staging
/// there. Copy/Move staging lands in a destination folder that the source
/// walk never visits when it is not itself a configured source, so this is
/// the sweep that actually reaches it; the walk covers configured source
/// roots the same way through `is_abandoned_leftover`. Best-effort and
/// shallow: a read failure here never stops the operation that called it.
pub fn sweep_private_tmp_leftovers(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|file_type| file_type.is_file())
            && is_private_tmp_name(&path)
            && is_abandoned_leftover(&path)
        {
            crate::fs_recovery::remove_file(&path, "private staging leftover cleanup");
        }
    }
}

/// Moves a private staging pathname into a fresh private hold and verifies the
/// physical file that actually moved against this operation's open
/// descriptor. This is the operation-owned claim used by both publication and
/// cleanup. A replacement is restored (or retained in the hold if its old name
/// was occupied again), never treated as ours.
pub fn claim_private(path: &Path, file: &File) -> io::Result<std::path::PathBuf> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private staging path has no parent",
        ));
    };
    for _ in 0..4 {
        let hold = parent.join(format!(
            "{CLAIM_PREFIX}{}-{}{PRIVATE_TMP_SUFFIX}",
            this_process_owner_tag(),
            crate::nanoid::generate().map_err(io::Error::other)?
        ));
        match crate::fs_publish::rename_no_replace(path, &hold) {
            Ok(()) => {
                if path_names_file(&hold, file) {
                    return Ok(hold);
                } else {
                    // The pathname was replaced before our claim. Put that
                    // file back when possible; otherwise leave it recoverable
                    // under the private hold rather than deleting a winner.
                    return match crate::fs_publish::rename_no_replace(&hold, path) {
                        Ok(()) => Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "private staging pathname was replaced",
                        )),
                        Err(restore_error) => Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!(
                                "private staging pathname was replaced; the replacement remains at {} because restoring it failed: {restore_error}",
                                hold.display()
                            ),
                        )),
                    };
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a private physical-claim pathname",
    ))
}

/// Best-effort cleanup of a private staging pathname. Callers never use this
/// for a public committed target; public targets are never unlinked as rollback.
pub fn remove_private_if_owned(path: &Path, file: &File) {
    match claim_private(path, file) {
        Ok(hold) => crate::fs_recovery::remove_file(&hold, "private staging cleanup"),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::AlreadyExists
            ) => {}
        Err(error) => crate::logging::warn(
            "private staging claim failed during cleanup",
            serde_json::json!({
                "path": path,
                "error": { "message": error.to_string() },
            }),
        ),
    }
}

/// One file this operation created under a private destination name, bound
/// to the descriptor that wrote it. Until it is published, dropping it removes
/// the private file, but only while that name still holds this file, so every
/// early return, failure, and cancellation abandons its private output.
#[derive(Debug)]
pub struct PrivateFile {
    path: std::path::PathBuf,
    file: File,
    published: bool,
}

impl PrivateFile {
    pub fn new(path: std::path::PathBuf, file: File) -> Self {
        Self {
            path,
            file,
            published: false,
        }
    }

    /// The private name, or the final name once published.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// Whether `path` currently names this file.
    pub fn is_named_by(&self, path: &Path) -> bool {
        path_names_file(path, &self.file)
    }

    /// Moves the file to a fresh private hold so no other writer can have
    /// replaced the name between this check and publication.
    pub fn claim(&mut self) -> io::Result<()> {
        self.path = claim_private(&self.path, &self.file)?;
        Ok(())
    }

    /// Publishes the file at `target` without replacing another entry. An
    /// occupied target answers `AlreadyExists` and the file stays private.
    pub fn publish(&mut self, target: &Path) -> io::Result<()> {
        crate::fs_publish::rename_no_replace(&self.path, target)?;
        self.path = target.to_path_buf();
        self.published = true;
        if !path_names_file(target, &self.file) {
            // Not `AlreadyExists`: the name was ours and is now someone
            // else's, which is a failure, not an occupied target to review.
            return Err(io::Error::other(
                format!(
                    "published output was replaced before completion: {}",
                    target.display()
                )));
        }
        Ok(())
    }
}

impl Drop for PrivateFile {
    fn drop(&mut self) {
        if !self.published {
            remove_private_if_owned(&self.path, &self.file);
        }
    }
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: `fingerprint_for` and
// `is_abandoned_leftover_against` are private helpers behind
// `is_abandoned_leftover`'s process-global public surface (one process-wide
// settled data root and one process-wide fingerprint). Exercising every case
// — two homes, a cloned home on a different host, an unsettled process — from
// the public API alone would need settling that process global differently
// per case, which a single test binary cannot do.
#[path = "../tests/unit/file_identity.rs"]
mod tests;

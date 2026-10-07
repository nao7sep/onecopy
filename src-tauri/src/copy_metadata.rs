//! Ordinary copy permissions and modified time, per content-lifecycle-conventions.
//! Both halves run inside `VolumeFile::with`, on the descriptor-owning worker.

use std::fs::{File, Permissions};
use std::io;
use std::time::SystemTime;

pub(crate) struct SourceMetadata {
    modified: SystemTime,
    permissions: Permissions,
}

impl SourceMetadata {
    pub(crate) fn read(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            modified: metadata.modified()?,
            permissions: metadata.permissions(),
        })
    }

    /// Fails only when the modified time cannot be set.
    pub(crate) fn apply(&self, file: &File) -> io::Result<()> {
        file.set_times(std::fs::FileTimes::new().set_modified(self.modified))?;
        apply_permissions(file, &self.permissions);
        Ok(())
    }
}

pub(crate) fn apply_replacement(source: &File, replacement: &File) -> io::Result<()> {
    apply_permissions(replacement, &source.metadata()?.permissions()); // data root
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

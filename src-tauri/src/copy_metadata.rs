//! Ordinary copy permissions and modified time, per content-lifecycle-conventions.
//! Both halves run inside `VolumeFile::with`, on the descriptor-owning worker.

use std::fs::{File, FileTimes, Permissions};
use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The earliest and latest modified times every FAT-family volume holds in
/// any time zone: FAT and exFAT store local time between 1980 and 2107.
const FAT_EARLIEST_SECS: u64 = 315_619_200; // 1980-01-02T00:00:00Z
const FAT_LATEST_SECS: u64 = 4_354_646_400; // 2107-12-30T00:00:00Z
/// FAT stores modified times to the even second.
const FAT_PRECISION: Duration = Duration::from_secs(2);

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
        file.set_times(FileTimes::new().set_modified(self.modified))?;
        // A volume that cannot hold the time stores another one without
        // failing: on macOS, FAT32 wraps a time outside 1980-2107 (1969
        // becomes 2105) and exFAT clamps it. Such a copy gets the nearest time
        // those volumes hold instead of an unrelated one.
        if let Some(nearest) = representable_fallback(self.modified, file.metadata()?.modified()?) {
            file.set_times(FileTimes::new().set_modified(nearest))?;
        }
        apply_permissions(file, &self.permissions);
        Ok(())
    }
}

/// The time to set instead when a volume stored `stored` for `requested`:
/// `requested` brought inside the FAT range, or `None` when the volume kept
/// it (within FAT's precision) or it was already inside the range.
pub(crate) fn representable_fallback(requested: SystemTime, stored: SystemTime) -> Option<SystemTime> {
    let apart = requested
        .duration_since(stored)
        .unwrap_or_else(|earlier| earlier.duration());
    if apart <= FAT_PRECISION {
        return None;
    }
    let nearest = requested.clamp(
        UNIX_EPOCH + Duration::from_secs(FAT_EARLIEST_SECS),
        UNIX_EPOCH + Duration::from_secs(FAT_LATEST_SECS),
    );
    (nearest != requested).then_some(nearest)
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

#[cfg(test)]
#[path = "../tests/unit/copy_metadata.rs"]
mod tests;

//! Hard working-set gates at the allocation boundary.
//!
//! The derived runtime admits one heavy class at a time. Independent native
//! image-preview jobs may share that class, so admission combines the
//! per-decode ceiling with live memory and CPU budgets. The limits are
//! deliberately not user-configurable: a setting that can disable OOM
//! protection is not a preference.

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageReader, Limits};

// Release measurement puts a 108 MiB 6144×6144 native still at the one-second
// pause boundary on Apple Silicon. Larger stills already have one bounded,
// process-supervised ffmpeg route, so keeping the native ceiling below that
// measured edge avoids a second image-worker architecture and lowers aggregate
// concurrency too.
//
// The physical Windows release probe found a much lower boundary on the Intel
// notebook: 1600×1600 RGB completed in 298 ms, while 1664×1664 took 1.35 s.
// 7.5 MiB admits the former decoded buffer and sends the latter through the
// same supervised ffmpeg fallback instead of making pause wait on native code.
#[cfg(windows)]
pub const MAX_DECODE_ALLOC: u64 = 15 * 512 * 1024;
#[cfg(not(windows))]
pub const MAX_DECODE_ALLOC: u64 = 96 * 1024 * 1024;
const IMAGE_JOB_RESERVATION: u64 = 2 * MAX_DECODE_ALLOC;
const IMAGE_CONCURRENCY_HEADROOM: u64 = 1024 * 1024 * 1024;
pub const MAX_FFMPEG_STILL_OUTPUT: usize = 256 * 1024 * 1024;
pub const MAX_PCM_OUTPUT: usize = 512 * 1024 * 1024;
pub const PCM_REQUIRED_AVAILABLE: u64 = 768 * 1024 * 1024;
pub const FACE_REQUIRED_AVAILABLE: u64 = 512 * 1024 * 1024;
pub const WHISPER_REQUIRED_AVAILABLE: u64 = 3 * 1024 * 1024 * 1024;
pub const SIMILARITY_REQUIRED_AVAILABLE: u64 = 512 * 1024 * 1024;
pub const SAFETY_ERROR_PREFIX: &str = "resource safety: ";
pub const DECODE_LIMIT_PREFIX: &str = "decode safety: ";

fn image_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits
}

pub fn decode_file(path: &Path) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    reader.limits(image_limits());
    reader.decode().map_err(decode_error)
}

pub fn decode_bytes(bytes: &[u8]) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    reader.limits(image_limits());
    reader.decode().map_err(decode_error)
}

fn decode_error(error: image::ImageError) -> String {
    if matches!(error, image::ImageError::Limits(_)) {
        format!(
            "{DECODE_LIMIT_PREFIX}image decode exceeds OneCopy's {} MiB safety limit",
            MAX_DECODE_ALLOC / 1024 / 1024
        )
    } else {
        error.to_string()
    }
}

pub fn is_decode_limit(error: &str) -> bool {
    error.starts_with(DECODE_LIMIT_PREFIX)
}

pub fn require_available(bytes: u64, operation: &str) -> Result<(), String> {
    let Some(available) = available_memory_bytes() else {
        return Err(format!(
            "{SAFETY_ERROR_PREFIX}{operation} is paused because available memory could not be measured safely"
        ));
    };
    if available < bytes {
        return Err(format!(
            "{SAFETY_ERROR_PREFIX}{operation} needs at least {} MiB of available memory; {} MiB is available",
            bytes / 1024 / 1024,
            available / 1024 / 1024
        ));
    }
    Ok(())
}

pub fn is_safety_error(error: &str) -> bool {
    error.starts_with(SAFETY_ERROR_PREFIX)
}

pub fn safety_message(error: &str) -> &str {
    error.strip_prefix(SAFETY_ERROR_PREFIX).unwrap_or(error)
}

/// Maximum native image conversions that may be in flight together now.
///
/// A decode may briefly hold both its source-sized pixel allocation and an
/// oriented copy, so each worker reserves twice the decoder's hard ceiling.
/// Unknown memory falls back to one worker. While the user is active, no more
/// than half the logical CPUs are admitted; idle work may use all but one.
pub(crate) fn image_worker_capacity(idle: bool) -> usize {
    let logical_cpus = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    image_worker_capacity_for(logical_cpus, available_memory_bytes(), idle)
}

fn image_worker_capacity_for(
    logical_cpus: usize,
    available_memory: Option<u64>,
    idle: bool,
) -> usize {
    let logical_cpus = logical_cpus.max(1);
    let cpu_capacity = if idle {
        logical_cpus.saturating_sub(1).max(1)
    } else {
        (logical_cpus / 2).max(1)
    };
    let memory_capacity = available_memory
        .map(|available| {
            let workers = available
                .saturating_sub(IMAGE_CONCURRENCY_HEADROOM)
                .checked_div(IMAGE_JOB_RESERVATION)
                .unwrap_or(0);
            usize::try_from(workers).unwrap_or(usize::MAX)
        })
        .unwrap_or(1)
        .max(1);
    cpu_capacity.min(memory_capacity).max(1)
}

#[cfg(target_os = "macos")]
fn available_memory_bytes() -> Option<u64> {
    unsafe extern "C" {
        fn mach_host_self() -> libc::mach_port_t;
    }
    let mut statistics = std::mem::MaybeUninit::<libc::vm_statistics64_data_t>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: `statistics` has the exact HOST_VM_INFO64 layout and `count`
    // names its writable integer slots. The Mach call initializes the value
    // only when it returns KERN_SUCCESS, which is checked before assume_init.
    let result = unsafe {
        libc::host_statistics64(
            mach_host_self(),
            libc::HOST_VM_INFO64,
            statistics.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS {
        return None;
    }
    // SAFETY: successful host_statistics64 initialized the structure above.
    let statistics = unsafe { statistics.assume_init() };
    // SAFETY: sysconf has no memory preconditions.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return None;
    }
    let reclaimable_pages = u64::from(statistics.free_count)
        .saturating_add(u64::from(statistics.inactive_count))
        .saturating_add(u64::from(statistics.speculative_count))
        .saturating_add(u64::from(statistics.purgeable_count));
    Some(reclaimable_pages.saturating_mul(page_size as u64))
}

#[cfg(windows)]
fn available_memory_bytes() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: `status` is a correctly-sized writable MEMORYSTATUSEX.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.avail_phys)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn available_memory_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::{image_worker_capacity_for, IMAGE_JOB_RESERVATION};

    #[test]
    fn active_image_work_leaves_cpu_headroom() {
        let abundant = Some(64 * IMAGE_JOB_RESERVATION);
        assert_eq!(image_worker_capacity_for(8, abundant, false), 4);
        assert_eq!(image_worker_capacity_for(8, abundant, true), 7);
    }

    #[test]
    fn image_concurrency_falls_back_to_one_when_memory_is_unknown_or_tight() {
        assert_eq!(image_worker_capacity_for(32, None, true), 1);
        assert_eq!(image_worker_capacity_for(32, Some(1024), true), 1);
    }

    #[test]
    fn image_concurrency_obeys_the_aggregate_memory_budget() {
        let for_three_workers = 1024 * 1024 * 1024 + 3 * IMAGE_JOB_RESERVATION;
        assert_eq!(
            image_worker_capacity_for(32, Some(for_three_workers), true),
            3
        );
    }
}

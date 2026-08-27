//! Hard working-set gates at the allocation boundary.
//!
//! The derived runtime admits only one class at a time, so these per-job
//! ceilings combine into a fixed aggregate bound rather than multiplying by
//! concurrency. They are deliberately not user-configurable: a setting that
//! can disable OOM protection is not a preference.

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, ImageReader, Limits};

pub const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;
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

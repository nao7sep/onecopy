//! Optional libheif acceleration for HEIC/HEIF/AVIF stills (Design: Stack
//! and platforms — "a system libheif, when present, as an optional
//! acceleration of the ffmpeg still-decode route").
//!
//! NEVER a hard dependency: the library is probed by dlopen at first use,
//! absence is the common case and costs one probe, and ANY failure — load,
//! bind, or decode — falls back to the ffmpeg route unconditionally. The
//! minimal C surface is bound by hand; every call that can fail returns
//! libheif's by-value error struct, checked before anything is trusted.
//!
//! Orientation: libheif applies the container's display transforms (irot,
//! imir) during decode, exactly as ffmpeg does — the committed orientation
//! fixtures pin that the two routes agree, colour-position and all.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::path::Path;
use std::sync::OnceLock;

use image::DynamicImage;

#[repr(C)]
#[derive(Clone, Copy)]
struct HeifError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

impl HeifError {
    fn check(self, what: &str) -> Result<(), String> {
        if self.code == 0 {
            return Ok(());
        }
        let detail = if self.message.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(self.message) }
                .to_string_lossy()
                .into_owned()
        };
        Err(format!("libheif {what} failed ({}): {detail}", self.code))
    }
}

const HEIF_COLORSPACE_RGB: c_int = 1;
const HEIF_CHROMA_INTERLEAVED_RGB: c_int = 10;
const HEIF_CHANNEL_INTERLEAVED: c_int = 10;

/// The bound library: the handful of functions the decode needs, resolved
/// once. Function pointers are copied out so the Library can live alongside.
struct Bound {
    _library: libloading::Library,
    context_alloc: unsafe extern "C" fn() -> *mut c_void,
    context_free: unsafe extern "C" fn(*mut c_void),
    read_from_memory: unsafe extern "C" fn(*mut c_void, *const u8, usize, *const c_void) -> HeifError,
    get_primary_handle: unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HeifError,
    handle_release: unsafe extern "C" fn(*mut c_void),
    decode_image: unsafe extern "C" fn(*mut c_void, *mut *mut c_void, c_int, c_int, *const c_void) -> HeifError,
    image_release: unsafe extern "C" fn(*mut c_void),
    image_get_width: unsafe extern "C" fn(*const c_void, c_int) -> c_int,
    image_get_height: unsafe extern "C" fn(*const c_void, c_int) -> c_int,
    get_plane_readonly: unsafe extern "C" fn(*const c_void, c_int, *mut c_int) -> *const u8,
}

unsafe impl Send for Bound {}
unsafe impl Sync for Bound {}

static BOUND: OnceLock<Option<Bound>> = OnceLock::new();

fn candidates() -> Vec<&'static str> {
    if cfg!(target_os = "macos") {
        vec![
            "/opt/homebrew/lib/libheif.dylib",
            "/usr/local/lib/libheif.dylib",
            "libheif.dylib",
        ]
    } else if cfg!(windows) {
        vec!["libheif.dll", "heif.dll"]
    } else {
        vec!["libheif.so.1", "libheif.so"]
    }
}

fn bind() -> Option<Bound> {
    for candidate in candidates() {
        let Ok(library) = (unsafe { libloading::Library::new(candidate) }) else {
            continue;
        };
        // Any missing symbol abandons this candidate — an old or exotic
        // build is treated as absent, never half-used.
        let bound = unsafe {
            let resolved = (|| -> Result<[*const c_void; 10], libloading::Error> {
                let mut get =
                    |name: &[u8]| library.get::<*const c_void>(name).map(|s| *s);
                Ok([
                    get(b"heif_context_alloc\0")?,
                    get(b"heif_context_free\0")?,
                    get(b"heif_context_read_from_memory_without_copy\0")?,
                    get(b"heif_context_get_primary_image_handle\0")?,
                    get(b"heif_image_handle_release\0")?,
                    get(b"heif_decode_image\0")?,
                    get(b"heif_image_release\0")?,
                    get(b"heif_image_get_width\0")?,
                    get(b"heif_image_get_height\0")?,
                    get(b"heif_image_get_plane_readonly\0")?,
                ])
            })();
            resolved.map(|symbols| Bound {
                context_alloc: std::mem::transmute(symbols[0]),
                context_free: std::mem::transmute(symbols[1]),
                read_from_memory: std::mem::transmute(symbols[2]),
                get_primary_handle: std::mem::transmute(symbols[3]),
                handle_release: std::mem::transmute(symbols[4]),
                decode_image: std::mem::transmute(symbols[5]),
                image_release: std::mem::transmute(symbols[6]),
                image_get_width: std::mem::transmute(symbols[7]),
                image_get_height: std::mem::transmute(symbols[8]),
                get_plane_readonly: std::mem::transmute(symbols[9]),
                _library: library,
            })
        };
        if let Ok(bound) = bound {
            crate::logging::debug(
                "libheif bound",
                serde_json::json!({ "path": candidate }),
            );
            return Some(bound);
        }
    }
    None
}

/// Whether a system libheif is present and bindable (probed once).
pub fn available() -> bool {
    BOUND.get_or_init(bind).is_some()
}

/// Decodes a HEIF-family file through the system libheif. Every failure is a
/// plain Err the caller answers with the ffmpeg fallback.
pub fn decode(path: &Path) -> Result<DynamicImage, String> {
    let Some(bound) = BOUND.get_or_init(bind).as_ref() else {
        return Err("libheif not present".to_string());
    };
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;

    unsafe {
        let context = (bound.context_alloc)();
        if context.is_null() {
            return Err("libheif context alloc failed".to_string());
        }
        // Everything below funnels through `finish` so the context is always
        // freed, success or failure.
        let result = (|| -> Result<DynamicImage, String> {
            (bound.read_from_memory)(context, bytes.as_ptr(), bytes.len(), std::ptr::null())
                .check("read")?;
            let mut handle: *mut c_void = std::ptr::null_mut();
            (bound.get_primary_handle)(context, &mut handle).check("primary handle")?;
            let decoded = (|| -> Result<DynamicImage, String> {
                let mut img: *mut c_void = std::ptr::null_mut();
                (bound.decode_image)(
                    handle,
                    &mut img,
                    HEIF_COLORSPACE_RGB,
                    HEIF_CHROMA_INTERLEAVED_RGB,
                    std::ptr::null(),
                )
                .check("decode")?;
                let result = (|| -> Result<DynamicImage, String> {
                    let width = (bound.image_get_width)(img, HEIF_CHANNEL_INTERLEAVED);
                    let height = (bound.image_get_height)(img, HEIF_CHANNEL_INTERLEAVED);
                    if width <= 0 || height <= 0 {
                        return Err("libheif reported empty image".to_string());
                    }
                    let mut stride: c_int = 0;
                    let plane =
                        (bound.get_plane_readonly)(img, HEIF_CHANNEL_INTERLEAVED, &mut stride);
                    if plane.is_null() || stride <= 0 {
                        return Err("libheif returned no pixel plane".to_string());
                    }
                    let (width, height, stride) =
                        (width as usize, height as usize, stride as usize);
                    let mut rgb = Vec::with_capacity(width * height * 3);
                    for row in 0..height {
                        let line = std::slice::from_raw_parts(plane.add(row * stride), width * 3);
                        rgb.extend_from_slice(line);
                    }
                    let buffer =
                        image::RgbImage::from_raw(width as u32, height as u32, rgb)
                            .ok_or("libheif plane size mismatch")?;
                    Ok(DynamicImage::ImageRgb8(buffer))
                })();
                (bound.image_release)(img);
                result
            })();
            (bound.handle_release)(handle);
            decoded
        })();
        (bound.context_free)(context);
        result
    }
}

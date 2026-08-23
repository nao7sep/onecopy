//! Built-in default extension sets (lowercase, no dot). These seed the editable
//! lists in `config.json` on first run; runtime classification reads the config's
//! copies, so a user can extend support without a rebuild. Companion extensions
//! are never primary items (the pairing design): RAW rides with its JPEG or
//! stands alone as an other-file.

pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "hif", "webp", "gif", "bmp", "tif", "tiff", "avif",
];

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "avi", "mts", "m2ts", "mkv", "webm", "3gp", "wmv", "mpg", "mpeg",
];

pub const COMPANION_EXTENSIONS: &[&str] = &[
    "thm", "lrv", "xmp", "aae", "arw", "cr2", "cr3", "dng", "nef", "orf", "rw2", "raf",
];

/// The three primary kinds plus companions; anything unlisted is an other-file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Image,
    Video,
    Companion,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Companion => "companion",
            Kind::Other => "other",
        }
    }
}

/// Classifies a lowercase extension against explicit lists (normally the
/// config's editable copies; the built-ins above are their first-run values).
pub fn classify(ext: &str, images: &[String], videos: &[String], companions: &[String]) -> Kind {
    let matches = |list: &[String]| list.iter().any(|e| e == ext);
    if matches(images) {
        Kind::Image
    } else if matches(videos) {
        Kind::Video
    } else if matches(companions) {
        Kind::Companion
    } else {
        Kind::Other
    }
}

/// Extracts the lowercase extension of a file name (no dot); empty when none.
pub fn lowercase_ext(file_name: &str) -> String {
    std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

//! Derived-image generation: the hash-keyed cache of grid thumbnails and
//! screen-fit previews that every view reads instead of original files. One
//! cache entry serves every copy of a file (the key is the content hash), and
//! the whole tree is reconstructible, so it is never backed up and may be
//! deleted freely.
//!
//! Layout under the cache root (config `cacheDir`, default `<root>/cache`):
//! `thumbs/<h2>/<hash>.webp` and `previews/<h2>/<hash>.webp`, sharded by the
//! hash's first two characters so no directory grows unbounded.
//!
//! Writes are atomic (temp sibling + rename) but deliberately NOT recorded:
//! cache derivatives are binary and reconstructible, outside the data-backup
//! conventions' managed text (not recorded: cache derivative, by design).
//!
//! Sharpness (Laplacian variance on the preview-sized grayscale) is computed
//! here because the decoded pixels are already in hand — it orders a similar
//! group best-first later, advisory only.
//!
//! HEIC/HEIF decoding is a separate task (libheif); until it lands those files
//! fail to decode here and surface as issues, never silent skips.

use std::path::{Path, PathBuf};

use image::DynamicImage;
use rusqlite::{params, Connection};

use crate::logging;
use crate::nanoid;

pub struct CachePaths {
    root: PathBuf,
}

impl CachePaths {
    pub fn new(root: PathBuf) -> CachePaths {
        CachePaths { root }
    }

    pub fn root_dir(&self) -> &Path {
        &self.root
    }

    fn shard(hash: &str) -> &str {
        hash.get(0..2).unwrap_or("00")
    }

    pub fn thumb(&self, hash: &str) -> PathBuf {
        self.root
            .join("thumbs")
            .join(Self::shard(hash))
            .join(format!("{hash}.webp"))
    }

    pub fn preview(&self, hash: &str) -> PathBuf {
        self.root
            .join("previews")
            .join(Self::shard(hash))
            .join(format!("{hash}.webp"))
    }
}

pub struct DerivedFacts {
    pub width: u32,
    pub height: u32,
    pub sharpness: f64,
    pub phash: u64,
}

/// Decodes one image, applies its EXIF orientation, writes the thumbnail and
/// preview cache entries, and returns the oriented dimensions + sharpness.
pub fn generate_for_image(
    src: &Path,
    hash: &str,
    cache: &CachePaths,
    thumb_edge: u32,
    preview_long_edge: u32,
) -> Result<DerivedFacts, String> {
    let orientation = read_orientation(src);
    let decoded = image::open(src).map_err(|e| e.to_string())?;
    let oriented = apply_orientation(decoded, orientation);
    let (width, height) = (oriented.width(), oriented.height());

    // Preview first (higher quality resize), thumbnail from the preview so the
    // original is traversed once and the thumb resize input is already small.
    let preview = fit_long_edge(&oriented, preview_long_edge, image::imageops::FilterType::CatmullRom);
    let sharpness = laplacian_variance(&preview.to_luma8());
    let phash = dhash(&preview);

    // An image that already fits the preview edge needs no resize — and when
    // its own format is one the webview displays as-is and no orientation
    // transform applied, a full-size re-encode adds nothing: the preview
    // entry becomes a byte-copy of the original (the .webp cache name is
    // load-bearing, so the serving protocol sniffs the real content type).
    if width.max(height) <= preview_long_edge && orientation == 1 && displayable_as_is(src) {
        copy_file_atomic(src, &cache.preview(hash))?;
    } else {
        write_webp(&preview, &cache.preview(hash), 80.0)?;
    }

    let thumb = fit_long_edge(&preview, thumb_edge, image::imageops::FilterType::Triangle);
    write_webp(&thumb, &cache.thumb(hash), 78.0)?;

    Ok(DerivedFacts {
        width,
        height,
        sharpness,
        phash,
    })
}

/// Formats the webview renders directly, making the preview byte-copy safe.
/// HEIC/TIFF/AVIF and friends stay on the WebP encode path.
fn displayable_as_is(src: &Path) -> bool {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif")
}

/// Atomic byte-copy into the cache (temp sibling + rename), the no-re-encode
/// counterpart of `write_webp`. not recorded: cache derivative (binary,
/// reconstructible), outside the managed-text backup path by design.
fn copy_file_atomic(src: &Path, target: &Path) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "cache path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cache");
    let tmp = parent.join(format!("{stem}-{}.tmp", nanoid::generate()));
    std::fs::copy(src, &tmp).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(())
}

/// A 64-bit difference hash: grayscale 9×8, one bit per horizontal neighbor
/// comparison. Hand-rolled (the img_hash crate pins an older image version);
/// Hamming distance over these bits is the similarity comparator the config's
/// `similarityPhashMaxDistance` names.
pub fn dhash(img: &DynamicImage) -> u64 {
    let small = img
        .resize_exact(9, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    let mut bits: u64 = 0;
    let mut bit = 0u32;
    for y in 0..8 {
        for x in 0..8 {
            if small.get_pixel(x, y).0[0] < small.get_pixel(x + 1, y).0[0] {
                bits |= 1 << bit;
            }
            bit += 1;
        }
    }
    bits
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct DeriveStats {
    pub derived: u64,
    pub failed: u64,
}

/// The pending pass: derive cache entries for image contents not yet derived.
/// One representative non-missing path per hash supplies the pixels; a decode
/// failure records an issue and marks the row failed so it is not retried
/// every run (a rescan that changes the file resets the marker via the
/// changed-row reset in the walk).
///
/// The decode/encode work runs on rayon across chunks (SQLite writes stay on
/// this thread), `progress` — when given — reports (done, total) after each
/// chunk so a long pass is visibly alive, and the scan cancel flag is honored
/// between chunks: derived rows keep their checkpoint, undone rows resume on
/// the next pass.
pub fn derive_images_pending(
    conn: &Connection,
    cache: &CachePaths,
    thumb_edge: u32,
    preview_long_edge: u32,
    progress: Option<&dyn Fn(u64, u64)>,
) -> Result<DeriveStats, String> {
    use rayon::prelude::*;

    let mut stats = DeriveStats::default();

    let mut stmt = conn
        .prepare(
            "SELECT c.hash, (SELECT p.abs_path FROM paths p \
             WHERE p.content_hash = c.hash AND p.missing = 0 LIMIT 1) \
             FROM contents c WHERE c.kind = 'image' AND c.derived_at_utc IS NULL",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .filter_map(|(hash, path)| path.map(|p| (hash, p)))
        .collect();
    drop(stmt);

    let total = rows.len() as u64;
    let mut done = 0u64;

    for chunk in rows.chunks(32) {
        if crate::scanner::cancelled() {
            return Err(crate::scanner::CANCELLED.to_string());
        }

        let results: Vec<(&String, &String, Result<DerivedFacts, String>)> = chunk
            .par_iter()
            .map(|(hash, path)| {
                if crate::scanner::cancelled() {
                    (hash, path, Err(crate::scanner::CANCELLED.to_string()))
                } else {
                    (
                        hash,
                        path,
                        generate_for_image(
                            Path::new(path),
                            hash,
                            cache,
                            thumb_edge,
                            preview_long_edge,
                        ),
                    )
                }
            })
            .collect();

        for (hash, path, outcome) in results {
            match outcome {
                Ok(facts) => {
                    stats.derived += 1;
                    conn.execute(
                        "UPDATE contents SET width = COALESCE(width, ?2), \
                         height = COALESCE(height, ?3), sharpness = ?4, phash = ?5, \
                         derived_at_utc = ?6 WHERE hash = ?1",
                        params![
                            hash,
                            facts.width,
                            facts.height,
                            facts.sharpness,
                            facts.phash as i64,
                            logging::now_iso_millis()
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                }
                Err(err) if err == crate::scanner::CANCELLED => {
                    // Skipped by the cancel — no checkpoint, no issue; the
                    // row stays pending for the next pass.
                }
                Err(err) => {
                    stats.failed += 1;
                    conn.execute(
                        "INSERT INTO issues (path, kind, message, created_at_utc) \
                         VALUES (?1, 'decode-error', ?2, ?3)",
                        params![path, err, logging::now_iso_millis()],
                    )
                    .map_err(|e| e.to_string())?;
                    conn.execute(
                        "UPDATE contents SET derived_at_utc = 'failed' WHERE hash = ?1",
                        [hash],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }

        done += chunk.len() as u64;
        if let Some(report) = progress {
            report(done.min(total), total);
        }
    }

    Ok(stats)
}

/// The cache's own subtrees under a root — the ONLY directories a cache move
/// copies or deletes. The root itself may be a user-picked folder holding
/// unrelated content, so tree-wide operations never touch anything else.
pub const CACHE_SUBTREES: [&str; 3] = ["thumbs", "previews", "strips"];

/// Copies every cache entry from `old_root` to `new_root` (same layout),
/// reporting (copied_bytes, total_bytes) as it goes, and size-verifying each
/// copy. Verification is size-equality: the tree is reconstructible (a
/// corrupt entry re-derives on its next miss), so a byte-hash read-back
/// would double the IO to protect data that self-heals anyway. Stranded
/// `.tmp` staging files are not carried over.
pub fn move_cache_tree(
    old_root: &Path,
    new_root: &Path,
    progress: &dyn Fn(u64, u64),
) -> Result<u64, String> {
    let mut files: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    let mut total = 0u64;
    for sub in CACHE_SUBTREES {
        let tree = old_root.join(sub);
        if !tree.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&tree).follow_links(false) {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.file_name().to_string_lossy().ends_with(".tmp") {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(old_root)
                .map_err(|e| e.to_string())?
                .to_path_buf();
            let size = entry.metadata().map_err(|e| e.to_string())?.len();
            total += size;
            files.push((entry.path().to_path_buf(), new_root.join(rel), size));
        }
    }

    let mut copied = 0u64;
    progress(0, total);
    for (src, dst, size) in files {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let written = std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
        if written != size {
            return Err(format!(
                "size mismatch copying {} ({written} of {size} bytes)",
                src.display()
            ));
        }
        copied += size;
        progress(copied, total);
    }
    Ok(copied)
}

/// Removes the cache subtrees under `root` (and the root itself when that
/// leaves it empty) — never anything else that may live beside them.
pub fn remove_cache_subtrees(root: &Path) {
    for sub in CACHE_SUBTREES {
        let _ = std::fs::remove_dir_all(root.join(sub));
    }
    let _ = std::fs::remove_dir(root);
}

/// Best-effort removal of one hash's cache entries — the synchronous half of
/// cache GC, called when the last path bearing the hash leaves the index.
pub fn remove_entries(cache: &CachePaths, hash: &str) {
    let _ = std::fs::remove_file(cache.thumb(hash));
    let _ = std::fs::remove_file(cache.preview(hash));
}

/// Startup sweep — the crash-leftover half of cache GC: deletes cache entries
/// whose hash is no longer in `contents`, plus stranded `.tmp` staging files
/// (safe: the single-instance app has no writer running at startup, and the
/// whole tree is reconstructible). Touches only the cache tree and the DB.
pub fn startup_sweep(conn: &Connection, cache: &CachePaths) -> Result<u64, String> {
    let mut removed = 0u64;
    let mut exists = conn
        .prepare("SELECT 1 FROM contents WHERE hash = ?1")
        .map_err(|e| e.to_string())?;

    for sub in ["thumbs", "previews"] {
        let tree = cache.root.join(sub);
        if !tree.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&tree).follow_links(false) {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let orphan = if let Some(hash) = name.strip_suffix(".webp") {
                !exists
                    .exists([hash])
                    .map_err(|e| e.to_string())?
            } else {
                // Stranded temps (or anything foreign) in a reconstructible tree.
                name.ends_with(".tmp")
            };
            if orphan && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Resizes to fit within `long_edge` on the longer side, never upscaling.
fn fit_long_edge(img: &DynamicImage, long_edge: u32, filter: image::imageops::FilterType) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let longest = w.max(h);
    if longest <= long_edge {
        return img.clone();
    }
    let scale = f64::from(long_edge) / f64::from(longest);
    let nw = ((f64::from(w) * scale).round() as u32).max(1);
    let nh = ((f64::from(h) * scale).round() as u32).max(1);
    img.resize_exact(nw, nh, filter)
}

/// EXIF orientation (1–8) via nom-exif; 1 (or unreadable) means as-stored.
fn read_orientation(src: &Path) -> u16 {
    match nom_exif::read_exif(src) {
        Ok(exif) => match exif.get(nom_exif::ExifTag::Orientation) {
            Some(nom_exif::EntryValue::U16(v)) => *v,
            Some(nom_exif::EntryValue::U32(v)) => *v as u16,
            _ => 1,
        },
        Err(_) => 1,
    }
}

/// Applies the eight EXIF orientations.
fn apply_orientation(img: DynamicImage, orientation: u16) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// Variance of the 3×3 Laplacian over a grayscale image — the classic cheap
/// blur metric: within one similar-shot group, higher = sharper.
fn laplacian_variance(luma: &image::GrayImage) -> f64 {
    let (w, h) = luma.dimensions();
    if w < 3 || h < 3 {
        return 0.0;
    }
    let px = |x: u32, y: u32| f64::from(luma.get_pixel(x, y).0[0]);
    let mut sum = 0.0;
    let mut sum_sq = 0.0;
    let n = f64::from((w - 2) * (h - 2));
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let lap = 4.0 * px(x, y) - px(x - 1, y) - px(x + 1, y) - px(x, y - 1) - px(x, y + 1);
            sum += lap;
            sum_sq += lap * lap;
        }
    }
    let mean = sum / n;
    sum_sq / n - mean * mean
}

/// Atomic cache write: temp sibling + rename. not recorded: cache derivative
/// (binary, reconstructible), outside the managed-text backup path by design.
/// Public within the crate: the video pipeline funnels strip frames through
/// this same single WebP encode path.
pub fn write_webp(img: &DynamicImage, target: &Path, quality: f32) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "cache path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    let rgba = img.to_rgba8();
    let encoded =
        webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height()).encode(quality);

    let stem = target
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cache");
    let tmp = parent.join(format!("{stem}-{}.tmp", nanoid::generate()));
    std::fs::write(&tmp, &*encoded).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_store;

    fn gradient_jpeg(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn generates_thumb_and_preview_within_limits_preserving_aspect() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-preview-")
            .tempdir()
            .unwrap();
        let src = gradient_jpeg(dir.path(), "big.jpg", 2000, 1000);
        let cache = CachePaths::new(dir.path().join("cache"));

        let facts = generate_for_image(&src, "abcd1234", &cache, 320, 1600).unwrap();
        assert_eq!((facts.width, facts.height), (2000, 1000));

        let preview = image::open(cache.preview("abcd1234")).unwrap();
        assert_eq!((preview.width(), preview.height()), (1600, 800));

        let thumb = image::open(cache.thumb("abcd1234")).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (320, 160));

        // Sharded layout: thumbs/ab/abcd1234.webp.
        assert!(cache
            .thumb("abcd1234")
            .to_string_lossy()
            .contains(&format!("thumbs{}ab{}", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR)));
    }

    #[test]
    fn small_images_are_never_upscaled_and_skip_the_reencode() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-preview-small-")
            .tempdir()
            .unwrap();
        let src = gradient_jpeg(dir.path(), "small.jpg", 200, 100);
        let cache = CachePaths::new(dir.path().join("cache"));

        generate_for_image(&src, "ffff0000", &cache, 320, 1600).unwrap();

        // Fits the preview edge + displayable format + no orientation: the
        // preview entry is a byte-copy of the original, not a WebP re-encode
        // (the .webp cache name is load-bearing; the protocol sniffs bytes).
        assert_eq!(
            std::fs::read(cache.preview("ffff0000")).unwrap(),
            std::fs::read(&src).unwrap()
        );
        let preview = image::ImageReader::open(cache.preview("ffff0000"))
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((preview.width(), preview.height()), (200, 100));
    }

    #[test]
    fn orientation_transforms_swap_dimensions_where_they_should() {
        let img = DynamicImage::new_rgb8(40, 20);
        assert_eq!(apply_orientation(img.clone(), 1).dimensions_tuple(), (40, 20));
        assert_eq!(apply_orientation(img.clone(), 3).dimensions_tuple(), (40, 20));
        assert_eq!(apply_orientation(img.clone(), 6).dimensions_tuple(), (20, 40));
        assert_eq!(apply_orientation(img, 8).dimensions_tuple(), (20, 40));
    }

    // Small helper so the orientation test reads naturally.
    trait DimTuple {
        fn dimensions_tuple(&self) -> (u32, u32);
    }
    impl DimTuple for DynamicImage {
        fn dimensions_tuple(&self) -> (u32, u32) {
            (self.width(), self.height())
        }
    }

    #[test]
    fn dhash_known_answers_pin_the_bit_layout() {
        // Strictly increasing brightness left-to-right: every neighbor
        // comparison is "left < right", so every bit is set. Decreasing:
        // none. These are hand-derivable reference vectors, not
        // implementation echoes.
        let rising = DynamicImage::ImageRgb8(image::RgbImage::from_fn(90, 80, |x, _| {
            let v = (x * 2) as u8;
            image::Rgb([v, v, v])
        }));
        assert_eq!(dhash(&rising), u64::MAX);

        let falling = DynamicImage::ImageRgb8(image::RgbImage::from_fn(90, 80, |x, _| {
            let v = 200 - (x * 2) as u8;
            image::Rgb([v, v, v])
        }));
        assert_eq!(dhash(&falling), 0);

        // A flat image is degenerate by design: zero hash, distance 0 to
        // every other flat image — which is why the group-size cap exists.
        let flat = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb([128; 3])));
        assert_eq!(dhash(&flat), 0);
    }

    #[test]
    fn dhash_survives_scaling_but_not_rotation() {
        let scene = |w: u32, h: u32| {
            DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
                // An asymmetric gradient-plus-blob scene, scale-independent.
                let fx = f64::from(x) / f64::from(w);
                let fy = f64::from(y) / f64::from(h);
                let blob = if (fx - 0.3).powi(2) + (fy - 0.6).powi(2) < 0.04 { 100.0 } else { 0.0 };
                let v = (fx * 155.0 + blob).min(255.0) as u8;
                image::Rgb([v, v, v])
            }))
        };
        let big = scene(800, 600);
        let small = scene(200, 150);
        let dist_scaled = (dhash(&big) ^ dhash(&small)).count_ones();
        assert!(dist_scaled <= 4, "same scene at two scales must match: {dist_scaled}");

        let rotated = big.rotate90();
        let dist_rotated = (dhash(&big) ^ dhash(&rotated)).count_ones();
        assert!(
            dist_rotated > 4,
            "a 90-degree rotation must not silently match: {dist_rotated}"
        );
    }

    #[test]
    fn sharp_images_score_higher_than_their_blurred_versions() {
        let sharp = image::RgbImage::from_fn(200, 200, |x, _| {
            if (x / 10) % 2 == 0 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 0, 0])
            }
        });
        let sharp_dyn = DynamicImage::ImageRgb8(sharp);
        let blurred = sharp_dyn.blur(4.0);
        let s_sharp = laplacian_variance(&sharp_dyn.to_luma8());
        let s_blur = laplacian_variance(&blurred.to_luma8());
        assert!(
            s_sharp > s_blur * 2.0,
            "sharp {s_sharp} should clearly exceed blurred {s_blur}"
        );
    }

    #[test]
    fn sweep_removes_orphans_and_temps_but_keeps_live_entries() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-sweep-")
            .tempdir()
            .unwrap();
        let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let cache = CachePaths::new(dir.path().join("cache"));
        conn.execute(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('live01', 1, 'image')",
            [],
        )
        .unwrap();

        for hash in ["live01", "orphan"] {
            for path in [cache.thumb(hash), cache.preview(hash)] {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, b"webp-bytes").unwrap();
            }
        }
        let stray_tmp = cache.thumb("live01").with_file_name("live01-xyz.tmp");
        std::fs::write(&stray_tmp, b"partial").unwrap();

        let removed = startup_sweep(&conn, &cache).unwrap();
        assert_eq!(removed, 3); // orphan thumb + orphan preview + stray tmp
        assert!(cache.thumb("live01").exists());
        assert!(cache.preview("live01").exists());
        assert!(!cache.thumb("orphan").exists());
        assert!(!stray_tmp.exists());

        // remove_entries drops a live pair on demand (the synchronous half).
        remove_entries(&cache, "live01");
        assert!(!cache.thumb("live01").exists());
        assert!(!cache.preview("live01").exists());
    }

    #[test]
    fn cache_move_copies_subtrees_verifies_sizes_and_spares_bystanders() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-cachemove-")
            .tempdir()
            .unwrap();
        let old_root = dir.path().join("old");
        let new_root = dir.path().join("new");
        let cache = CachePaths::new(old_root.clone());
        for (path, bytes) in [
            (cache.thumb("aa11"), b"thumb-bytes".as_slice()),
            (cache.preview("aa11"), b"preview-bytes-longer"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        // A stranded temp is not carried; a bystander file outside the cache
        // subtrees is neither copied nor deleted.
        std::fs::write(cache.thumb("aa11").with_file_name("aa11-x.tmp"), b"partial").unwrap();
        std::fs::write(old_root.join("bystander.txt"), b"not ours").unwrap();

        let reports: std::cell::RefCell<Vec<(u64, u64)>> = std::cell::RefCell::new(Vec::new());
        let moved =
            move_cache_tree(&old_root, &new_root, &|c, t| reports.borrow_mut().push((c, t)))
                .unwrap();
        assert_eq!(moved, 11 + 20);
        let reports = reports.into_inner();
        assert_eq!(reports.first(), Some(&(0, 31)));
        assert_eq!(reports.last(), Some(&(31, 31)));

        let new_cache = CachePaths::new(new_root.clone());
        assert_eq!(std::fs::read(new_cache.thumb("aa11")).unwrap(), b"thumb-bytes");
        assert!(!new_cache.thumb("aa11").with_file_name("aa11-x.tmp").exists());

        remove_cache_subtrees(&old_root);
        assert!(!old_root.join("thumbs").exists());
        assert!(
            old_root.join("bystander.txt").exists(),
            "a user-picked folder's own content must survive"
        );
    }

    #[test]
    fn derive_pending_processes_images_once_and_flags_decode_failures() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-derive-")
            .tempdir()
            .unwrap();
        let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let cache = CachePaths::new(dir.path().join("cache"));

        let good = gradient_jpeg(dir.path(), "good.jpg", 800, 600);
        let bad = dir.path().join("bad.jpg");
        std::fs::write(&bad, b"not a jpeg at all").unwrap();

        conn.execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('good01', 1, 'image');
             INSERT INTO contents (hash, byte_size, kind) VALUES ('bad001', 1, 'image');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
               VALUES ('{}', '{}', 'good.jpg', 'image', 'good01');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
               VALUES ('{}', '{}', 'bad.jpg', 'image', 'bad001');",
            good.display(),
            dir.path().display(),
            bad.display(),
            dir.path().display(),
        ))
        .unwrap();

        let stats = derive_images_pending(&conn, &cache, 320, 1600, None).unwrap();
        assert_eq!((stats.derived, stats.failed), (1, 1));
        assert!(cache.preview("good01").exists());

        let issue_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM issues WHERE kind = 'decode-error'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(issue_count, 1);

        // A second pass has nothing left to do — failures are not retried.
        let again = derive_images_pending(&conn, &cache, 320, 1600, None).unwrap();
        assert_eq!((again.derived, again.failed), (0, 0));

        // The good row carries dimensions + sharpness.
        let (w, h): (i64, i64) = conn
            .query_row(
                "SELECT width, height FROM contents WHERE hash = 'good01'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((w, h), (800, 600));
    }
}

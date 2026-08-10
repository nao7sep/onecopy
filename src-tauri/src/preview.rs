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
    derive_from_decoded(decoded, orientation, src, hash, cache, thumb_edge, preview_long_edge)
}

/// The tee variant for a provisionally-identified image: the derive was going
/// to read every byte anyway, so the REAL full hash comes free — read once,
/// hash the bytes, decode from memory, and write the cache under the real
/// key. Returns (real_hash, facts); the caller promotes the identity.
pub fn generate_for_image_teeing(
    src: &Path,
    cache: &CachePaths,
    thumb_edge: u32,
    preview_long_edge: u32,
) -> Result<(String, DerivedFacts), String> {
    let bytes = std::fs::read(src).map_err(|e| e.to_string())?;
    let real_hash = blake3::hash(&bytes).to_hex().to_string();
    let orientation = read_orientation(src);
    let decoded = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let facts = derive_from_decoded(
        decoded,
        orientation,
        src,
        &real_hash,
        cache,
        thumb_edge,
        preview_long_edge,
    )?;
    Ok((real_hash, facts))
}

fn derive_from_decoded(
    decoded: DynamicImage,
    orientation: u16,
    src: &Path,
    hash: &str,
    cache: &CachePaths,
    thumb_edge: u32,
    preview_long_edge: u32,
) -> Result<DerivedFacts, String> {
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

        type DeriveOutcome = Result<(Option<String>, DerivedFacts), String>;
        let results: Vec<(&String, &String, DeriveOutcome)> = chunk
            .par_iter()
            .map(|(hash, path)| {
                if crate::scanner::cancelled() {
                    (hash, path, Err(crate::scanner::CANCELLED.to_string()))
                } else if crate::scanner::is_provisional(hash) {
                    // The decode reads every byte anyway — tee the REAL hash
                    // out of the same read and derive under it; the writer
                    // below promotes the identity.
                    (
                        hash,
                        path,
                        generate_for_image_teeing(
                            Path::new(path),
                            cache,
                            thumb_edge,
                            preview_long_edge,
                        )
                        .map(|(real, facts)| (Some(real), facts)),
                    )
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
                        )
                        .map(|facts| (None, facts)),
                    )
                }
            })
            .collect();

        for (hash, path, outcome) in results {
            match outcome {
                Ok((promoted, facts)) => {
                    let key = match promoted {
                        Some(real) => {
                            crate::scanner::promote_identity(conn, cache, hash, &real)?;
                            real
                        }
                        None => hash.clone(),
                    };
                    stats.derived += 1;
                    conn.execute(
                        "UPDATE contents SET width = COALESCE(width, ?2), \
                         height = COALESCE(height, ?3), sharpness = ?4, phash = ?5, \
                         derived_at_utc = ?6 WHERE hash = ?1",
                        params![
                            key,
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

/// Moves one identity's cache entries to a new key (provisional→real
/// promotion): thumb, preview, and any strip frames. Best-effort — a missing
/// source has nothing to move, and the startup sweep collects strays.
pub fn rename_entries(cache: &CachePaths, old: &str, new: &str, strip_frames: i64) {
    let mut moves = vec![
        (cache.thumb(old), cache.thumb(new)),
        (cache.preview(old), cache.preview(new)),
    ];
    for i in 0..strip_frames.max(0) as u32 {
        moves.push((
            crate::video::strip_path(cache, old, i),
            crate::video::strip_path(cache, new, i),
        ));
    }
    for (from, to) in moves {
        if from.exists() {
            if let Some(parent) = to.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::rename(&from, &to);
        }
    }
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

    // EXCEPTION to the tests-live-in-tests/ rule (tests-folder
    // conventions, Rust form): orientation transforms and the Laplacian
    // sharpness metric are private pixel math — promoting them would
    // widen the module's surface just to test through it.

    #[test]
    fn orientation_transforms_swap_dimensions_where_they_should() {
        let img = DynamicImage::new_rgb8(40, 20);
        assert_eq!(apply_orientation(img.clone(), 1).dimensions_tuple(), (40, 20));
        assert_eq!(apply_orientation(img.clone(), 3).dimensions_tuple(), (40, 20));
        assert_eq!(apply_orientation(img.clone(), 6).dimensions_tuple(), (20, 40));
        assert_eq!(apply_orientation(img, 8).dimensions_tuple(), (20, 40));
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

    // Small helper so the orientation test reads naturally.
    trait DimTuple {
        fn dimensions_tuple(&self) -> (u32, u32);
    }
    impl DimTuple for DynamicImage {
        fn dimensions_tuple(&self) -> (u32, u32) {
            (self.width(), self.height())
        }
    }

}

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
    let decoded = image::open(src).map_err(|e| e.to_string())?;
    let oriented = apply_orientation(decoded, read_orientation(src));
    let (width, height) = (oriented.width(), oriented.height());

    // Preview first (higher quality resize), thumbnail from the preview so the
    // original is traversed once and the thumb resize input is already small.
    let preview = fit_long_edge(&oriented, preview_long_edge, image::imageops::FilterType::CatmullRom);
    let sharpness = laplacian_variance(&preview.to_luma8());
    let phash = dhash(&preview);
    write_webp(&preview, &cache.preview(hash), 80.0)?;

    let thumb = fit_long_edge(&preview, thumb_edge, image::imageops::FilterType::Triangle);
    write_webp(&thumb, &cache.thumb(hash), 78.0)?;

    Ok(DerivedFacts {
        width,
        height,
        sharpness,
        phash,
    })
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
pub fn derive_images_pending(
    conn: &Connection,
    cache: &CachePaths,
    thumb_edge: u32,
    preview_long_edge: u32,
) -> Result<DeriveStats, String> {
    let mut stats = DeriveStats::default();

    let mut stmt = conn
        .prepare(
            "SELECT c.hash, (SELECT p.abs_path FROM paths p \
             WHERE p.content_hash = c.hash AND p.missing = 0 LIMIT 1) \
             FROM contents c WHERE c.kind = 'image' AND c.derived_at_utc IS NULL",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    for (hash, path) in rows {
        let Some(path) = path else { continue };
        match generate_for_image(Path::new(&path), &hash, cache, thumb_edge, preview_long_edge) {
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
                    [&hash],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(stats)
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
fn write_webp(img: &DynamicImage, target: &Path, quality: f32) -> Result<(), String> {
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
    fn small_images_are_never_upscaled() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-preview-small-")
            .tempdir()
            .unwrap();
        let src = gradient_jpeg(dir.path(), "small.jpg", 200, 100);
        let cache = CachePaths::new(dir.path().join("cache"));

        generate_for_image(&src, "ffff0000", &cache, 320, 1600).unwrap();
        let preview = image::open(cache.preview("ffff0000")).unwrap();
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

        let stats = derive_images_pending(&conn, &cache, 320, 1600).unwrap();
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
        let again = derive_images_pending(&conn, &cache, 320, 1600).unwrap();
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

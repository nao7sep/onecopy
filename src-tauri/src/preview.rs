//! Derived-image generation: the hash-keyed cache of grid thumbnails and
//! screen-fit previews that every view reads instead of original files. One
//! cache entry serves every copy of a file (the key is the content hash), and
//! the whole tree is reconstructible, so it is never backed up and may be
//! deleted freely.
//!
//! Layout under `<root>/cache`:
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
//! The formats the `image` crate cannot open — the HEIF family and AVIF —
//! decode through the managed ffmpeg instead (no system dependency on the
//! default path). Without ffmpeg those files are left BLOCKED rather than
//! failed, so installing it later derives them instead of stranding them.

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

    /// Full-resolution conversion of a format the webview cannot paint
    /// (HEIC/AVIF), decoded on demand for the 100% view. PNG: lossless, so
    /// pixel-peeping stays honest; reconstructible like every other entry.
    pub fn fullres(&self, hash: &str) -> PathBuf {
        self.root
            .join("fullres")
            .join(Self::shard(hash))
            .join(format!("{hash}.png"))
    }

    /// A video's on-demand transcript (Design: Video handling) — derived
    /// data like everything else here, keyed by the content hash.
    pub fn transcript(&self, hash: &str) -> PathBuf {
        self.root
            .join("transcripts")
            .join(Self::shard(hash))
            .join(format!("{hash}.txt"))
    }
}

pub struct DerivedFacts {
    pub width: u32,
    pub height: u32,
    pub sharpness: f64,
    pub phash: u64,
}

/// Marks an image row whose format needs the managed ffmpeg while ffmpeg is
/// absent. Distinct from `failed`: the file is fine and nothing about it has
/// to change — installing ffmpeg is enough for the next pass to derive it.
pub const NEEDS_FFMPEG: &str = "needs-ffmpeg";

/// The derive pipeline's output version. Bump it when a change makes existing
/// cache entries wrong (a different thumbnail geometry, a corrected
/// orientation rule, a new phash) — every row stamped with an older version
/// becomes pending again on the next pass. Nothing on disk is touched: the
/// cache is reconstructible by definition.
///
/// 2: the analysis luminance composites alpha over mid-gray (dhash and
/// sharpness previously read the RGB hidden under transparent pixels), so
/// every stored phash for an alpha-bearing image is wrong until re-derived.
/// 3: rows gained the CLIP embedding; re-deriving lets the embed pass find
/// every image pending once the similarity model is installed.
pub const DERIVE_VERSION: i64 = 3;

/// Extensions the `image` crate cannot decode, which route through the
/// managed ffmpeg instead. Measured against image 0.25 and ffmpeg 9.0
/// (2026-08-14): the crate rejects heic/heif/hif as an unrecognized format
/// and reports AVIF unsupported, while ffmpeg decodes all four. Every one of
/// them is a declared supported extension, so without this route they are
/// permanent decode failures.
pub fn needs_ffmpeg_decode(src: &Path) -> bool {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "heic" | "heif" | "hif" | "avif")
}

/// Decodes one still through the managed ffmpeg, for the formats above.
///
/// The frame comes back over a pipe as BMP and nothing is staged on disk.
/// BMP is bit-identical to ffmpeg's PNG output (verified by PSNR) and 3.8×
/// faster to produce on photographic content — 379 ms against 1437 ms for a
/// 12 MP still — because neither side pays a compression pass.
///
/// ffmpeg applies the file's display orientation ITSELF, from HEIF `irot`
/// and from EXIF alike, and `-noautorotate` does not suppress it. The frame
/// therefore arrives upright and its EXIF orientation must NOT be applied a
/// second time: an Apple HEIC carries both the `irot` property and a
/// matching EXIF `Orientation` describing the SAME rotation, so re-applying
/// it would turn every rotated photo a further 90°.
fn decode_via_ffmpeg(ffmpeg: &Path, src: &Path) -> Result<DynamicImage, String> {
    decode_via_ffmpeg_bounded(ffmpeg, src, None)
}

/// `max_edge` scales INSIDE ffmpeg (after its own display-orientation pass),
/// never upscaling. The derive path uses it (Phase 33): a 48 MP HEIC piped as
/// full-size BMP is ~144 MB of transfer, allocation and BMP parse per photo —
/// at 1600 px it is ~6 MB, a 25× cut in everything after the decode. The
/// decode itself is untouched (HEVC has no partial decode), and the original
/// DIMENSIONS are safe: the metadata pass owns width/height and the derive's
/// UPDATE goes through COALESCE. The 100% view keeps the unbounded route.
fn decode_via_ffmpeg_bounded(
    ffmpeg: &Path,
    src: &Path,
    max_edge: Option<u32>,
) -> Result<DynamicImage, String> {
    logging::debug(
        "ffmpeg invocation",
        serde_json::json!({ "op": "decode-still", "src": src.to_string_lossy() }),
    );
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-frames:v", "1"]);
    if let Some(edge) = max_edge {
        // min() so a small image never upscales; -2 keeps dimensions even.
        cmd.args(["-vf", &format!("scale='min({edge},iw)':-2")]);
    }
    cmd.args(["-f", "image2pipe", "-c:v", "bmp", "-"]);
    let run = crate::subprocess::run_bounded(cmd, &crate::scanner::cancelled)?;
    if !run.status_ok || run.stdout.is_empty() {
        // The recent-output tail, bounded — the whole point is diagnosing
        // this one file, not carrying an ffmpeg essay into a DB column.
        return Err(format!(
            "ffmpeg could not decode this format: {}",
            run.stderr_tail()
        ));
    }
    image::load_from_memory(&run.stdout).map_err(|e| e.to_string())
}

/// Decodes `src`, returning the image alongside the EXIF orientation still
/// to apply — always 1 for an ffmpeg decode, which arrives upright already.
/// Public as the routing seam the decode-path tests drive directly.
pub fn decode_image(src: &Path, ffmpeg: Option<&Path>) -> Result<(DynamicImage, u16), String> {
    if needs_ffmpeg_decode(src) {
        // ONE decode route (Phase 33): the managed ffmpeg. A system libheif
        // used to accelerate this in-process, but a second route the
        // developer never exercises daily is drift waiting to happen, and it
        // cost a dlopen module, a kill switch, and a four-path test matrix.
        let ffmpeg = ffmpeg.ok_or_else(|| "ffmpeg is not installed".to_string())?;
        return Ok((decode_via_ffmpeg(ffmpeg, src)?, 1));
    }
    let decoded = image::open(src).map_err(|e| e.to_string())?;
    Ok((decoded, read_orientation(src)))
}

/// Decodes one image, applies its EXIF orientation, writes the thumbnail and
/// preview cache entries, and returns the oriented dimensions + sharpness.
pub fn generate_for_image(
    src: &Path,
    hash: &str,
    cache: &CachePaths,
    thumb_edge: u32,
    preview_long_edge: u32,
    ffmpeg: Option<&Path>,
) -> Result<DerivedFacts, String> {
    let (decoded, orientation) = if needs_ffmpeg_decode(src) {
        let ffmpeg = ffmpeg.ok_or_else(|| "ffmpeg is not installed".to_string())?;
        // The derive needs at most the preview edge; the bounded pipe is the
        // fast path (see decode_via_ffmpeg_bounded).
        (
            decode_via_ffmpeg_bounded(ffmpeg, src, Some(preview_long_edge))?,
            1,
        )
    } else {
        decode_image(src, ffmpeg)?
    };
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
    ffmpeg: Option<&Path>,
) -> Result<(String, DerivedFacts), String> {
    if needs_ffmpeg_decode(src) {
        // ffmpeg opens the file itself, so there is no read of ours to tee:
        // hash it streaming (never a whole-file buffer, which for a 12 MP
        // still is the larger cost) and let the decode read separately.
        let real_hash = crate::hashing::full_hash(src).map_err(|e| e.to_string())?;
        let ffmpeg_path = ffmpeg.ok_or_else(|| "ffmpeg is not installed".to_string())?;
        let (decoded, orientation) =
            (decode_via_ffmpeg_bounded(ffmpeg_path, src, Some(preview_long_edge))?, 1);
        let facts = derive_from_decoded(
            decoded,
            orientation,
            src,
            &real_hash,
            cache,
            thumb_edge,
            preview_long_edge,
        )?;
        return Ok((real_hash, facts));
    }

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
    let sharpness = laplacian_variance(&luma_for_analysis(&preview));
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
/// HEIC/TIFF/AVIF and friends stay on the WebP encode path — load-bearing for
/// the ffmpeg-decoded formats, since byte-copying a HEIC into the cache would
/// hand the webview the one thing it cannot display.
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
/// Luminance with alpha composited over MID-GRAY. `to_luma8` alone ignores
/// alpha, so the RGB hidden UNDER transparent pixels — pixels the user cannot
/// see — drove both the visual hash and the sharpness score: two
/// identical-looking icons could hash apart while different-looking ones
/// collided (the icon-corpus hairball was partly this). Mid-gray is the one
/// backdrop that keeps both white-on-transparent and black-on-transparent art
/// visible; photos carry no alpha and take the plain path untouched.
pub fn luma_for_analysis(img: &DynamicImage) -> image::GrayImage {
    if !img.color().has_alpha() {
        return img.to_luma8();
    }
    let rgba = img.to_rgba8();
    image::GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y);
        let a = u32::from(p[3]);
        // The same Rec.709 weights `to_luma8` uses, then the alpha blend.
        let lum =
            (2126 * u32::from(p[0]) + 7152 * u32::from(p[1]) + 722 * u32::from(p[2])) / 10000;
        image::Luma([u8::try_from((lum * a + 128 * (255 - a)) / 255).unwrap_or(255)])
    })
}

pub fn dhash(img: &DynamicImage) -> u64 {
    // Composite BEFORE the resize: resizing RGBA blends the hidden RGB of
    // transparent pixels into their opaque neighbours (the filter does not
    // weight by alpha), so downscaling first re-leaks exactly the invisible
    // pixels the analysis luminance exists to exclude — the pinning test
    // caught this order.
    let small = image::imageops::resize(
        &luma_for_analysis(img),
        9,
        8,
        image::imageops::FilterType::Triangle,
    );
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

/// Ensures the full-resolution conversion for one HEIC/AVIF content exists,
/// decoding through the managed ffmpeg on first request. Its
/// command is declared `#[tauri::command(async)]` so it runs on the async
/// runtime — Tauri dispatches a plain command on the MAIN thread, where a
/// multi-hundred-millisecond decode stalls the compositor and the whole
/// window stops repainting. Never inside the protocol handler either, which
/// is synchronous on the main thread. Idempotent: an existing entry returns
/// at once.
pub fn ensure_fullres(
    conn: &Connection,
    cache: &CachePaths,
    ffmpeg: Option<&Path>,
    hash: &str,
) -> Result<(), String> {
    let target = cache.fullres(hash);
    if target.exists() {
        return Ok(());
    }
    let Some(ffmpeg) = ffmpeg else {
        return Err("ffmpeg is not installed — install it from Managed tools".to_string());
    };
    let path: String = conn
        .query_row(
            "SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
            [hash],
            |r| r.get(0),
        )
        .map_err(|_| "no live copy of this photo".to_string())?;
    // not recorded: full-resolution conversion is a reconstructible binary
    // cache entry, staged beside its final path.
    // ffmpeg applies the container's display transforms itself, so the
    // decode is already upright (one decode route, Phase 33).
    let src = Path::new(&path);
    let decoded = decode_via_ffmpeg(ffmpeg, src)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut bytes: Vec<u8> = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or("cache");
    let parent = target.parent().ok_or("cache path has no parent")?;
    let tmp = parent.join(format!("{stem}-{}.tmp", crate::nanoid::generate()));
    std::fs::write(&tmp, &bytes).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(())
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct DeriveStats {
    pub derived: u64,
    pub failed: u64,
    /// Rows left for a later pass because their format needs the managed
    /// ffmpeg and it is not installed — waiting, not broken.
    pub blocked_no_ffmpeg: u64,
}

/// The pending pass: derive cache entries for image contents not yet derived.
/// One representative non-missing path per hash supplies the pixels; a decode
/// failure records an issue and marks the row failed so it is not retried
/// every run (a rescan that changes the file resets the marker via the
/// changed-row reset in the walk).
///
/// A format needing the managed ffmpeg while ffmpeg is absent is marked
/// `needs-ffmpeg` instead of failed — no issue row, because nothing is wrong
/// with the file — and this pass picks those rows back up as soon as ffmpeg
/// is present, which is what makes the wizard's skippable offer honest.
///
/// The decode/encode work runs on rayon across chunks (SQLite writes stay on
/// this thread), `progress` — when given — reports (done, total) after each
/// chunk so a long pass is visibly alive, and the scan cancel flag is honored
/// between chunks: derived rows keep their checkpoint, undone rows resume on
/// the next pass.
/// The one success write-back for a derived image — the bulk pass and the
/// on-demand single-item derive both land here, so the checkpoint semantics
/// (facts, version stamp, retired decode-error issue) cannot drift between
/// the two routes.
fn record_derive_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    facts: &DerivedFacts,
) -> Result<(), String> {
    conn.execute(
        &format!(
            "UPDATE contents SET width = COALESCE(width, ?2), \
             height = COALESCE(height, ?3), sharpness = ?4, phash = ?5, \
             derived_at_utc = ?6, derived_version = {DERIVE_VERSION} \
             WHERE hash = ?1"
        ),
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
    // Current-state issues: a decode that now succeeds retires
    // the failure it recorded on an earlier pass.
    crate::index_store::clear_issues(conn, path, &["decode-error"])?;
    Ok(())
}

/// Derives ONE item's thumb + preview on demand — the user clicked a photo
/// the scan's bulk pass has not reached yet (it runs walk-order, and on a
/// slow machine the tail is hours away). Idempotent: an already-derived hash
/// returns immediately. Deliberately refuses a provisional hash — identity
/// promotion belongs to the scan that created it, and the pass will reach
/// the row anyway.
pub fn derive_one(
    conn: &Connection,
    cache: &CachePaths,
    thumb_edge: u32,
    preview_long_edge: u32,
    ffmpeg: Option<&Path>,
    hash: &str,
) -> Result<(), String> {
    if cache.thumb(hash).exists() && cache.preview(hash).exists() {
        return Ok(());
    }
    if crate::scanner::is_provisional(hash) {
        return Err("still being indexed — the scan will reach this photo".to_string());
    }
    let path: String = conn
        .query_row(
            "SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
            [hash],
            |r| r.get(0),
        )
        .map_err(|_| "no live copy of this photo".to_string())?;
    let src = Path::new(&path);
    if ffmpeg.is_none() && needs_ffmpeg_decode(src) {
        return Err(
            "this format needs the video & HEIC support — install it from Managed tools"
                .to_string(),
        );
    }
    let facts = generate_for_image(src, hash, cache, thumb_edge, preview_long_edge, ffmpeg)
        .map_err(|err| {
            // The same honesty as the bulk pass: a broken file is recorded,
            // not silently retried on every click.
            let _ = crate::index_store::upsert_issue(conn, Some(&path), "decode-error", &err);
            let _ = conn.execute(
                "UPDATE contents SET derived_at_utc = 'failed' WHERE hash = ?1",
                [hash],
            );
            err
        })?;
    record_derive_success(conn, hash, &path, &facts)
}

pub fn derive_images_pending(
    conn: &Connection,
    cache: &CachePaths,
    thumb_edge: u32,
    preview_long_edge: u32,
    ffmpeg: Option<&Path>,
    progress: Option<&dyn Fn(u64, u64)>,
) -> Result<DeriveStats, String> {
    use rayon::prelude::*;

    let mut stats = DeriveStats::default();

    // With ffmpeg present the rows it previously blocked come back into the
    // pass; without it they are left alone rather than re-marked every scan.
    // Stale means "produced by an older pipeline", which is a different thing
    // from failed (the FILE is broken — retrying it every scan is the churn the
    // sentinel exists to stop) and from needs-ffmpeg (blocked, and owned by the
    // ffmpeg branch below; without ffmpeg it must stay blocked, not be retried
    // into a failure).
    let stale = format!(
        "c.derived_version < {DERIVE_VERSION} \
         AND c.derived_at_utc NOT IN ('failed', '{NEEDS_FFMPEG}')"
    );
    let pending_clause = if ffmpeg.is_some() {
        format!("(c.derived_at_utc IS NULL OR c.derived_at_utc = '{NEEDS_FFMPEG}' OR ({stale}))")
    } else {
        format!("(c.derived_at_utc IS NULL OR ({stale}))")
    };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT c.hash, (SELECT p.abs_path FROM paths p \
             WHERE p.content_hash = c.hash AND p.missing = 0 LIMIT 1) \
             FROM contents c WHERE c.kind = 'image' AND {pending_clause}"
        ))
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
                } else if ffmpeg.is_none() && needs_ffmpeg_decode(Path::new(path)) {
                    (hash, path, Err(NEEDS_FFMPEG.to_string()))
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
                            ffmpeg,
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
                            ffmpeg,
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
                    record_derive_success(conn, &key, path, &facts)?;
                }
                Err(err) if err == crate::scanner::CANCELLED => {
                    // Skipped by the cancel — no checkpoint, no issue; the
                    // row stays pending for the next pass.
                }
                Err(err) if err == NEEDS_FFMPEG => {
                    // Waiting on a tool, not a bad file: no issue row, and
                    // the marker keeps it out of every pass until ffmpeg
                    // arrives (without it the startup resume would fire on
                    // work it cannot do, every launch).
                    stats.blocked_no_ffmpeg += 1;
                    conn.execute(
                        "UPDATE contents SET derived_at_utc = ?2 WHERE hash = ?1",
                        params![hash, NEEDS_FFMPEG],
                    )
                    .map_err(|e| e.to_string())?;
                }
                Err(err) => {
                    stats.failed += 1;
                    crate::index_store::upsert_issue(conn, Some(&path), "decode-error", &err)?;
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
        (cache.fullres(old), cache.fullres(new)),
        (cache.transcript(old), cache.transcript(new)),
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

    for sub in ["thumbs", "previews", "fullres", "transcripts"] {
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
            let hash_of = |n: &str| {
                n.strip_suffix(".webp")
                    .or_else(|| n.strip_suffix(".png"))
                    .or_else(|| n.strip_suffix(".txt"))
                    .map(str::to_string)
            };
            let orphan = if let Some(hash) = hash_of(&name) {
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

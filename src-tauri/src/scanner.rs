//! The indexing core: walk → stat → hash tiers → metadata/evidence → resolve,
//! all checkpointed into the index DB so an interrupted run resumes where it
//! stopped (unchanged size+mtime rows are skipped on the next pass). Symlinks
//! are not followed; hard links are distinct paths by design.
//!
//! Hashing is ONE ladder for every kind (no media exception): a unique size
//! reads nothing, a size collision gets the 64 KB prehash, and only prehash
//! collisions get the full hash — the single-copy user's library is hardly
//! read at all. Media with nothing to compare against get a PROVISIONAL
//! identity (`p<path_id>`) so the cache and UI have a key; it promotes to the
//! real hash wherever a full read happens anyway (image derive tees the
//! decode, move-out tees the copy) or the ladder later demands one. Same size
//! + same prehash + different full hash among supposed copies is recorded as
//! a `copies-disagree` issue.
//!
//! This module is synchronous and testable against temp trees; the
//! thread-spawning, progress-event-emitting wrapper arrives with the Phase 3
//! UI.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection, OptionalExtension};

use crate::extensions;
use crate::hashing;
use crate::logging;
use crate::metadata;
use crate::resolution::{self, ResolutionConfig};
use crate::timestamps;

/// Cooperative scan cancellation: set on app exit (and cleared at scan start),
/// checked inside every per-item pipeline loop so a quit interrupts the scan
/// in bounded time instead of killing it mid-write. A cancelled stage returns
/// the `CANCELLED` sentinel, which the scan wrapper reports as a cancellation,
/// never a failure — the checkpointed rows resume on the next launch.
pub static SCAN_CANCEL: AtomicBool = AtomicBool::new(false);

/// The sentinel a cancelled stage propagates in place of a real error.
pub const CANCELLED: &str = "scan cancelled";

pub fn cancelled() -> bool {
    SCAN_CANCEL.load(Ordering::Relaxed)
}

fn check_cancel() -> Result<(), String> {
    if cancelled() {
        Err(CANCELLED.to_string())
    } else {
        Ok(())
    }
}

/// The extension lists the scanner classifies against (the config's editable
/// copies).
pub struct ScanLists {
    pub images: Vec<String>,
    pub videos: Vec<String>,
    pub companions: Vec<String>,
}

/// Everything one scan run needs, projected out of the config JSON with the
/// typed defaults filling gaps — the store never validates, each consumer
/// projects what it needs (config-seeding conventions).
pub struct ScanSettings {
    pub source_dirs: Vec<String>,
    pub lists: ScanLists,
    pub resolution: ResolutionConfig,
    pub similarity: crate::similarity::SimilarityConfig,
    pub strip: crate::video::StripConfig,
    pub thumb_edge: u32,
    pub preview_long_edge: u32,
    pub keep_awake: bool,
    pub cache_root: std::path::PathBuf,
    /// The managed ffmpeg when present on disk; None leaves videos underived.
    pub ffmpeg: Option<std::path::PathBuf>,
    pub temp_dir: std::path::PathBuf,
}

pub fn settings_from_config(
    config: Option<&serde_json::Value>,
    data_root: &Path,
    now_ms: i64,
) -> ScanSettings {
    let defaults = crate::storage::DefaultConfig::default();
    let get = |key: &str| config.and_then(|c| c.get(key));

    let u32_of = |key: &str, fallback: u32| -> u32 {
        get(key)
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(fallback)
    };

    let tz: chrono_tz::Tz = get("defaultTimezone")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .or_else(|| defaults.default_timezone.parse().ok())
        .unwrap_or(chrono_tz::UTC);

    let cache_root = get("cacheDir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_root.join(crate::storage::CACHE_DIR_NAME));

    let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect();
    ScanSettings {
        source_dirs: string_list_preserving_case(config, "sourceDirs"),
        // Supported file types are specs, not user choices: the lists live in
        // extensions.rs only, and a stray legacy key in config.json is ignored.
        lists: ScanLists {
            images: owned(extensions::IMAGE_EXTENSIONS),
            videos: owned(extensions::VIDEO_EXTENSIONS),
            companions: owned(extensions::COMPANION_EXTENSIONS),
        },
        resolution: ResolutionConfig {
            default_timezone: tz,
            good_range_start_year: get("goodRangeStartYear")
                .and_then(|v| v.as_i64())
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(defaults.good_range_start_year),
            now_ms,
        },
        similarity: crate::similarity::SimilarityConfig {
            max_gap_seconds: u32_of("similarityMaxGapSeconds", defaults.similarity_max_gap_seconds),
            phash_max_distance: u32_of(
                "similarityPhashMaxDistance",
                defaults.similarity_phash_max_distance,
            ),
            max_group_size: u32_of(
                "similarityMaxGroupSize",
                defaults.similarity_max_group_size,
            ),
        },
        strip: crate::video::StripConfig {
            seconds_per_frame: u32_of(
                "videoStripSecondsPerFrame",
                defaults.video_strip_seconds_per_frame,
            ),
            min_frames: u32_of("videoStripMinFrames", defaults.video_strip_min_frames),
            max_frames: u32_of("videoStripMaxFrames", defaults.video_strip_max_frames),
        },
        ffmpeg: {
            let path = crate::binaries_manager::ffmpeg_path(data_root);
            path.is_file().then_some(path)
        },
        temp_dir: data_root.join(crate::binaries_manager::TEMP_DIR_NAME),
        thumb_edge: u32_of("thumbnailEdgePx", defaults.thumbnail_edge_px),
        preview_long_edge: u32_of("previewLongEdgePx", defaults.preview_long_edge_px),
        keep_awake: get("keepAwakeDuringIndexing")
            .and_then(|v| v.as_bool())
            .unwrap_or(defaults.keep_awake_during_indexing),
        cache_root,
    }
}

// Paths keep their case (unlike extensions, which normalize lowercase).
fn string_list_preserving_case(config: Option<&serde_json::Value>, key: &str) -> Vec<String> {
    config
        .and_then(|c| c.get(key))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Default, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub roots: u64,
    pub seen: u64,
    pub added: u64,
    pub full_hashed: u64,
    pub copies_disagree: u64,
    pub resolved: u64,
    pub undated: u64,
    pub paired: u64,
    pub derived: u64,
    pub derive_failed: u64,
    pub videos_derived: u64,
    pub similar_groups: u64,
}

/// One full pipeline run over every configured root: walk → hash → extract →
/// resolve → pair → derive, reporting a progress line after each stage.
pub fn run_full_scan(
    conn: &Connection,
    settings: &ScanSettings,
    progress: &dyn Fn(&str, String),
) -> Result<ScanSummary, String> {
    let mut summary = ScanSummary::default();

    for root in &settings.source_dirs {
        let stats = walk_root(conn, Path::new(root), &settings.lists)?;
        summary.roots += 1;
        summary.seen += stats.seen;
        summary.added += stats.added;
        progress(
            "walk",
            format!("{root}: {} files ({} new)", stats.seen, stats.added),
        );
    }

    run_pipeline_tail(conn, settings, progress, &mut summary)?;
    Ok(summary)
}

/// True when checkpointed work from an interrupted run is still waiting: media
/// rows never hashed, or image/video contents never derived. Videos count only
/// while ffmpeg is present — without it the video stage would skip them again,
/// and a resume that can do nothing must not fire on every launch. Cheap
/// (three indexed EXISTS probes), so callers may use it as a gate.
/// Whether any configured root still owes a full walk — it has never been
/// walked to completion, or a walk over it was interrupted. This is the one
/// thing `pending_work_exists` cannot see: its probes are all row-level, so
/// once the tail drains the rows a partial walk created, it reports clean
/// forever while whole directories remain unread.
pub fn walk_owed(conn: &Connection, roots: &[String]) -> Result<bool, String> {
    for root in roots {
        let complete: Option<bool> = conn
            .query_row(
                "SELECT last_completed_at_utc IS NOT NULL AND dirty = 0 \
                 FROM scan_dirs WHERE root = ?1",
                params![root],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if complete != Some(true) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn pending_work_exists(conn: &Connection, ffmpeg_present: bool) -> Result<bool, String> {
    let probe = |sql: &str| -> Result<bool, String> {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .map(|n| n != 0)
            .map_err(|e| e.to_string())
    };
    if probe(
        "SELECT EXISTS(SELECT 1 FROM paths WHERE missing = 0 AND content_hash IS NULL \
         AND kind IN ('image', 'video'))",
    )? {
        return Ok(true);
    }
    if probe(
        "SELECT EXISTS(SELECT 1 FROM contents WHERE kind = 'image' AND derived_at_utc IS NULL)",
    )? {
        return Ok(true);
    }
    // Stills blocked on ffmpeg become derivable the moment it lands, so they
    // are pending work then and inert before — the same gate the videos use.
    if ffmpeg_present
        && probe(&format!(
            "SELECT EXISTS(SELECT 1 FROM contents WHERE kind = 'image' \
             AND derived_at_utc = '{}')",
            crate::preview::NEEDS_FFMPEG
        ))?
    {
        return Ok(true);
    }
    if ffmpeg_present
        && probe(
            "SELECT EXISTS(SELECT 1 FROM contents WHERE kind = 'video' AND derived_at_utc IS NULL)",
        )?
    {
        return Ok(true);
    }
    Ok(false)
}

/// The pipeline minus the walk: hash → extract → resolve → pair → derive →
/// video → group, over whatever the checkpoints left pending. Shared by the
/// full scan, the startup resume, and the scoped section rescan, so every
/// recovery path runs the same (and the whole) tail.
pub fn run_pipeline_tail(
    conn: &Connection,
    settings: &ScanSettings,
    progress: &dyn Fn(&str, String),
    summary: &mut ScanSummary,
) -> Result<(), String> {
    let cache = crate::preview::CachePaths::new(settings.cache_root.clone());
    let hash_stats = hash_pending(conn, &cache)?;
    summary.full_hashed = hash_stats.full_hashed;
    summary.copies_disagree = hash_stats.copies_disagree;
    progress(
        "hash",
        format!(
            "{} hashed, {} unique ({} media identified without a read), {} disagreements",
            hash_stats.full_hashed,
            hash_stats.skipped_unique + hash_stats.provisional_created,
            hash_stats.provisional_created,
            hash_stats.copies_disagree
        ),
    );

    let extract_stats = extract_pending(conn)?;
    progress("extract", format!("{} files", extract_stats.extracted));

    let resolve_stats = resolve_from_evidence(conn, &settings.resolution, ResolveScope::PendingOnly)?;
    summary.resolved = resolve_stats.resolved;
    summary.undated = resolve_stats.undated;
    progress(
        "resolve",
        format!("{} resolved, {} undated", resolve_stats.resolved, resolve_stats.undated),
    );

    let pair_stats = pair_companions(conn)?;
    summary.paired = pair_stats.paired;
    progress("pair", format!("{} companions", pair_stats.paired));

    let cache = crate::preview::CachePaths::new(settings.cache_root.clone());
    let per_item = |done: u64, total: u64| progress("derive", format!("{done}/{total} previews"));
    let derive_stats = crate::preview::derive_images_pending(
        conn,
        &cache,
        settings.thumb_edge,
        settings.preview_long_edge,
        settings.ffmpeg.as_deref(),
        Some(&per_item),
    )?;
    summary.derived = derive_stats.derived;
    summary.derive_failed = derive_stats.failed;
    progress(
        "derive",
        if derive_stats.blocked_no_ffmpeg > 0 {
            format!(
                "{} previews, {} failures, {} waiting for ffmpeg",
                derive_stats.derived, derive_stats.failed, derive_stats.blocked_no_ffmpeg
            )
        } else {
            format!("{} previews, {} failures", derive_stats.derived, derive_stats.failed)
        },
    );

    let video_stats = crate::video::derive_videos_pending(
        conn,
        &cache,
        settings.ffmpeg.as_deref(),
        &settings.temp_dir,
        settings.thumb_edge,
        settings.preview_long_edge,
        &settings.strip,
    )?;
    summary.videos_derived = video_stats.derived;
    progress(
        "video",
        if video_stats.skipped_no_ffmpeg {
            "ffmpeg not installed — videos left for a later scan".to_string()
        } else {
            format!(
                "{} posters+strips, {} failures",
                video_stats.derived, video_stats.failed
            )
        },
    );

    let group_stats = crate::similarity::rebuild_groups(conn, &settings.similarity)?;
    summary.similar_groups = group_stats.groups;
    progress(
        "group",
        format!(
            "{} similar groups over {} photos",
            group_stats.groups, group_stats.grouped_items
        ),
    );

    Ok(())
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct WalkStats {
    pub seen: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub marked_missing: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Upsert {
    Added,
    Updated,
    Unchanged,
}

/// The one per-file upsert: stat + classify + insert/update/skip-unchanged.
/// Shared by the full walk and the watcher's single-directory re-stat, so the
/// two can never drift on the checkpoint semantics (size+mtime unchanged =
/// skip, changed = reset content facts).
pub fn upsert_file(
    conn: &Connection,
    path: &Path,
    lists: &ScanLists,
) -> Result<Upsert, String> {
    let abs = path.to_string_lossy().to_string();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let size = meta.len() as i64;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);
    let birthtime_ms = meta
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);

    let ext = extensions::lowercase_ext(&file_name);
    let kind = extensions::classify(&ext, &lists.images, &lists.videos, &lists.companions).as_str();
    let stem = Path::new(&file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&file_name)
        .to_lowercase();

    let existing: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT size, mtime_ms FROM paths WHERE abs_path = ?1",
            [&abs],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    match existing {
        Some((old_size, old_mtime)) if old_size == size && old_mtime == mtime_ms => {
            conn.execute(
                "UPDATE paths SET missing = 0 WHERE abs_path = ?1 AND missing = 1",
                [&abs],
            )
            .map_err(|e| e.to_string())?;
            Ok(Upsert::Unchanged)
        }
        Some(_) => {
            // A provisional key is `p<path_id>` — derived from the path, not
            // the bytes — so an in-place replacement regenerates the SAME key.
            // Left alone, the old contents row hands the new file the previous
            // file's facts: byte_size, phash, sharpness, strip_frames and,
            // fatally, derived_at_utc, which makes both derive passes skip it
            // for the life of the index. That is why re-saving a trimmed clip
            // kept showing the old poster and strip, and why a rescan did not
            // fix it. Captured here, dropped after the row detaches below —
            // paths.content_hash is a foreign key into contents, so deleting
            // first is a constraint violation.
            let stale_provisional: Option<String> = conn
                .query_row(
                    "SELECT content_hash FROM paths WHERE abs_path = ?1",
                    [&abs],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?
                .flatten()
                .filter(|hash| is_provisional(hash));
            conn.execute(
                "UPDATE paths SET size = ?2, mtime_ms = ?3, birthtime_ms = ?4, ext = ?5, \
                 kind = ?6, stem = ?7, prehash = NULL, content_hash = NULL, \
                 indexed_at_utc = NULL, resolved_utc_ms = NULL, resolved_source = NULL, \
                 date_only = 0, missing = 0 WHERE abs_path = ?1",
                params![abs, size, mtime_ms, birthtime_ms, ext, kind, stem],
            )
            .map_err(|e| e.to_string())?;
            // Only a row nothing else references: a provisional key that was
            // promoted and is now shared by real copies must survive.
            if let Some(hash) = stale_provisional {
                conn.execute(
                    "DELETE FROM contents WHERE hash = ?1 \
                       AND NOT EXISTS (SELECT 1 FROM paths WHERE content_hash = ?1)",
                    [&hash],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(Upsert::Updated)
        }
        None => {
            let dir_path = path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            conn.execute(
                "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, \
                 mtime_ms, birthtime_ms, missing) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)",
                params![abs, dir_path, file_name, stem, ext, kind, size, mtime_ms, birthtime_ms],
            )
            .map_err(|e| e.to_string())?;
            Ok(Upsert::Added)
        }
    }
}

/// Walks one source root: upserts every regular file as a `paths` row, skips
/// unchanged rows (same size + mtime — the checkpoint that makes rescans and
/// resumes cheap), resets content facts when a file changed, and marks rows
/// under the root that no longer exist as missing.
pub fn walk_root(conn: &Connection, root: &Path, lists: &ScanLists) -> Result<WalkStats, String> {
    let mut stats = WalkStats::default();
    let root_str = root.to_string_lossy().to_string();
    let scanned_at = logging::now_iso_millis();

    // Claim the root as walk-in-flight. `upsert_file` writes in autocommit, so
    // a cancelled walk leaves its prefix committed and the rest of the root
    // simply absent from `paths` — and nothing about a row can express "this
    // directory was never read". Only the completion write below clears this,
    // so an interrupted walk stays owed and the next launch re-walks instead
    // of running the tail over a permanently half-indexed library.
    conn.execute(
        "INSERT INTO scan_dirs (root, dirty) VALUES (?1, 1) \
         ON CONFLICT(root) DO UPDATE SET dirty = 1",
        params![root_str],
    )
    .map_err(|e| e.to_string())?;

    // Collect the currently-present set to diff against the DB afterwards.
    let mut present: Vec<String> = Vec::new();

    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        check_cancel()?;
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                record_issue(
                    conn,
                    err.path().map(|p| p.to_string_lossy().to_string()),
                    "walk-error",
                    &err.to_string(),
                )?;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let abs = path.to_string_lossy().to_string();
        // The app's own trash is never indexed.
        if abs.contains(".onecopy-trash") {
            continue;
        }

        stats.seen += 1;
        present.push(abs.clone());

        match upsert_file(conn, path, lists) {
            Ok(Upsert::Added) => stats.added += 1,
            Ok(Upsert::Updated) => stats.updated += 1,
            Ok(Upsert::Unchanged) => stats.unchanged += 1,
            Err(err) => {
                stats.seen -= 1;
                present.pop();
                record_issue(conn, Some(abs), "stat-error", &err)?;
            }
        }
    }

    // Anything under this root the walk did not see is missing. The rows stay
    // (their trash/delete history may matter) but leave every view and count.
    // LIKE wildcards in the root itself (`_` is common in real paths) are
    // escaped with `!`, which appears in no sane path on either OS.
    let escaped_root = root_str
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_");
    let placeholders_root = format!("{}%", ensure_trailing_separator(&escaped_root));
    let mut select = conn
        .prepare("SELECT abs_path FROM paths WHERE abs_path LIKE ?1 ESCAPE '!' AND missing = 0")
        .map_err(|e| e.to_string())?;
    let known: Vec<String> = select
        .query_map([&placeholders_root], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    let present_set: std::collections::HashSet<&String> = present.iter().collect();
    for path in known {
        if !present_set.contains(&path) {
            conn.execute("UPDATE paths SET missing = 1 WHERE abs_path = ?1", [&path])
                .map_err(|e| e.to_string())?;
            stats.marked_missing += 1;
        }
    }

    conn.execute(
        "INSERT INTO scan_dirs (root, last_completed_at_utc, dirty) VALUES (?1, ?2, 0) \
         ON CONFLICT(root) DO UPDATE SET last_completed_at_utc = ?2, dirty = 0",
        params![root_str, scanned_at],
    )
    .map_err(|e| e.to_string())?;

    Ok(stats)
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct HashStats {
    pub prehashed: u64,
    pub full_hashed: u64,
    pub skipped_unique: u64,
    pub provisional_created: u64,
    pub copies_disagree: u64,
    pub errors: u64,
}

/// Provisional content identity for a media file whose bytes were never read:
/// `p<path_id>` — unique per path by construction (so its copy count is 1),
/// and impossible to mistake for a real 64-hex blake3 hash. It exists so the
/// cache and the UI have a key before any full read happens, and it promotes
/// in place the first time a real hash appears (a size collision forcing the
/// ladder up, an image-derive tee, or a move-out tee).
pub fn provisional_key(path_id: i64) -> String {
    format!("p{path_id}")
}

pub fn is_provisional(hash: &str) -> bool {
    hash.starts_with('p')
}

/// Promotes a provisional identity to its real full hash: contents row,
/// paths pointers, similar-group membership, and the hash-keyed cache
/// entries all move to the real key. When the real hash already exists —
/// the provisional file turned out to be a copy of known content — the rows
/// merge instead (the established row's facts win; the provisional cache
/// entries are dropped and the startup sweep collects any strays).
pub fn promote_identity(
    conn: &Connection,
    cache: &crate::preview::CachePaths,
    provisional: &str,
    real_hash: &str,
) -> Result<(), String> {
    if !is_provisional(provisional) || provisional == real_hash {
        return Ok(());
    }
    let already_known: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM contents WHERE hash = ?1)",
            [real_hash],
            |r| r.get::<_, i64>(0).map(|n| n != 0),
        )
        .map_err(|e| e.to_string())?;

    if already_known {
        conn.execute(
            "UPDATE paths SET content_hash = ?2 WHERE content_hash = ?1",
            params![provisional, real_hash],
        )
        .map_err(|e| e.to_string())?;
        // Groups rebuild wholesale each scan; dropping the provisional
        // membership is enough (never duplicating the real row's).
        conn.execute(
            "DELETE FROM similar_group_members WHERE content_hash = ?1",
            [provisional],
        )
        .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM contents WHERE hash = ?1", [provisional])
            .map_err(|e| e.to_string())?;
        crate::preview::remove_entries(cache, provisional);
    } else {
        // The FK from paths forbids renaming the parent in place: copy the
        // row under the real key, repoint the children, drop the old row.
        conn.execute(
            "INSERT INTO contents (hash, byte_size, kind, phash, camera_make, camera_model, \
             width, height, duration_ms, sharpness, strip_frames, derived_at_utc) \
             SELECT ?2, byte_size, kind, phash, camera_make, camera_model, \
             width, height, duration_ms, sharpness, strip_frames, derived_at_utc \
             FROM contents WHERE hash = ?1",
            params![provisional, real_hash],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE paths SET content_hash = ?2 WHERE content_hash = ?1",
            params![provisional, real_hash],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE similar_group_members SET content_hash = ?2 WHERE content_hash = ?1",
            params![provisional, real_hash],
        )
        .map_err(|e| e.to_string())?;
        let strip_frames: Option<i64> = conn
            .query_row(
                "SELECT strip_frames FROM contents WHERE hash = ?1",
                [real_hash],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM contents WHERE hash = ?1", [provisional])
            .map_err(|e| e.to_string())?;
        crate::preview::rename_entries(cache, provisional, real_hash, strip_frames.unwrap_or(0));
    }
    Ok(())
}

/// The unified content ladder over rows without a REAL hash — one rule for
/// every kind, no media exception: a unique size reads nothing, a size
/// collision reads the 64 KB head+tail prehash, and only a prehash collision
/// reads the full blake3. Collapsing copies still requires full-hash
/// equality, always. Media with nothing to compare against get a PROVISIONAL
/// identity (the cache and UI need a key before any read; images promote to
/// a real hash for free at derive, where the decode reads every byte
/// anyway); other-files stay hash-less as before. A size matching an
/// ALREADY-HASHED content forces the full read directly — a late-arriving
/// copy of known content must collapse into it, or the copy-count health
/// check lies.
pub fn hash_pending(
    conn: &Connection,
    cache: &crate::preview::CachePaths,
) -> Result<HashStats, String> {
    let mut stats = HashStats::default();

    struct Row {
        id: i64,
        abs: String,
        size: i64,
        kind: String,
        provisional: Option<String>,
        prehash: Option<String>,
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, abs_path, size, kind, content_hash, prehash FROM paths \
             WHERE missing = 0 AND (content_hash IS NULL OR content_hash GLOB 'p*')",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                abs: r.get(1)?,
                size: r.get(2)?,
                kind: r.get(3)?,
                provisional: r.get(4)?,
                prehash: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    // Sizes of established (real-hashed) contents: a pending row matching one
    // goes straight to the full read.
    let mut known_stmt = conn
        .prepare("SELECT DISTINCT byte_size FROM contents WHERE NOT hash GLOB 'p*'")
        .map_err(|e| e.to_string())?;
    let known_sizes: std::collections::HashSet<i64> = known_stmt
        .query_map([], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(known_stmt);

    let is_media = |kind: &str| kind == "image" || kind == "video";

    // Assigns the resting identity of a row that nothing collides with.
    let settle_unique = |row: &Row, stats: &mut HashStats| -> Result<(), String> {
        if row.provisional.is_some() {
            return Ok(()); // already identified, still unique
        }
        if is_media(&row.kind) {
            let key = provisional_key(row.id);
            conn.execute(
                "INSERT INTO contents (hash, byte_size, kind) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(hash) DO NOTHING",
                params![key, row.size, row.kind],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE paths SET content_hash = ?2 WHERE id = ?1",
                params![row.id, key],
            )
            .map_err(|e| e.to_string())?;
            stats.provisional_created += 1;
        } else {
            stats.skipped_unique += 1;
        }
        Ok(())
    };

    // Full-hashes one row and lands its identity (promotion for provisional
    // rows, creation/collapse otherwise). Returns the hash for disagreement
    // accounting.
    let land_full_hash = |row: &Row, stats: &mut HashStats| -> Result<Option<String>, String> {
        match hashing::full_hash_cancellable(Path::new(&row.abs), &SCAN_CANCEL) {
            Ok(hash) => {
                stats.full_hashed += 1;
                if let Some(provisional) = &row.provisional {
                    promote_identity(conn, cache, provisional, &hash)?;
                } else {
                    store_content_hash(conn, row.id, &hash, row.size, &row.kind)?;
                }
                Ok(Some(hash))
            }
            Err(err) => {
                // A cancel that interrupted the read is a shutdown, never a
                // file problem — no issue row for it.
                check_cancel()?;
                stats.errors += 1;
                record_issue(conn, Some(row.abs.clone()), "read-error", &err.to_string())?;
                Ok(None)
            }
        }
    };

    let mut by_size: HashMap<i64, Vec<Row>> = HashMap::new();
    for row in rows {
        by_size.entry(row.size).or_default().push(row);
    }

    for (size, group) in by_size {
        check_cancel()?;
        if group.len() == 1 && !known_sizes.contains(&size) {
            settle_unique(&group[0], &mut stats)?;
            continue;
        }
        if known_sizes.contains(&size) {
            // Collides with established content: the prehash tier cannot
            // decide (established media were never prehashed) — read fully.
            for row in &group {
                check_cancel()?;
                let _ = land_full_hash(row, &mut stats)?;
            }
            continue;
        }
        // Size collision within the pending set: prehash each, then
        // full-hash only prehash collisions.
        let mut by_prehash: HashMap<String, Vec<Row>> = HashMap::new();
        for mut row in group {
            check_cancel()?;
            let pre = match &row.prehash {
                Some(pre) => Some(pre.clone()),
                None => match hashing::prehash(Path::new(&row.abs)) {
                    Ok(pre) => {
                        stats.prehashed += 1;
                        conn.execute(
                            "UPDATE paths SET prehash = ?2 WHERE id = ?1",
                            params![row.id, pre],
                        )
                        .map_err(|e| e.to_string())?;
                        Some(pre)
                    }
                    Err(err) => {
                        stats.errors += 1;
                        record_issue(conn, Some(row.abs.clone()), "read-error", &err.to_string())?;
                        None
                    }
                },
            };
            if let Some(pre) = pre {
                row.prehash = Some(pre.clone());
                by_prehash.entry(pre).or_default().push(row);
            }
        }
        for (_pre, collided) in by_prehash {
            if collided.len() == 1 {
                settle_unique(&collided[0], &mut stats)?;
                continue;
            }
            let group_len = collided.len();
            let mut hashes_in_group: Vec<String> = Vec::new();
            for row in &collided {
                check_cancel()?;
                if let Some(hash) = land_full_hash(row, &mut stats)? {
                    if !hashes_in_group.contains(&hash) {
                        hashes_in_group.push(hash);
                    }
                }
            }
            // Same size + same prehash + diverging full hashes: bit rot or a
            // divergent sync among supposed copies — surface it.
            if hashes_in_group.len() > 1 {
                stats.copies_disagree += 1;
                record_issue(
                    conn,
                    None,
                    "copies-disagree",
                    &format!(
                        "{group_len} same-size same-prehash files split into {} distinct contents (size {size})",
                        hashes_in_group.len()
                    ),
                )?;
            }
        }
    }

    Ok(stats)
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct ExtractStats {
    pub extracted: u64,
}

/// The evidence pass: reads in-file metadata (per kind) and runs the filename
/// tokenizer for rows not yet extracted, persisting each finding as a
/// serialized evidence row. This is the ONLY place resolution inputs touch a
/// file; after it, timezone/good-range/pattern changes re-resolve purely from
/// the DB.
pub fn extract_pending(conn: &Connection) -> Result<ExtractStats, String> {
    let mut stats = ExtractStats::default();

    let rows: Vec<(i64, String, String, String)> = collect_rows_4(
        conn,
        "SELECT id, abs_path, file_name, kind FROM paths \
         WHERE missing = 0 AND indexed_at_utc IS NULL",
    )?;

    for (id, abs, file_name, kind) in rows {
        check_cancel()?;
        let path = Path::new(&abs);
        let meta = match kind.as_str() {
            "image" => Some(metadata::read_image_metadata(path)),
            "video" => Some(metadata::read_video_metadata(path)),
            // Companion RAW files are TIFF containers with readable EXIF.
            "companion" => Some(metadata::read_image_metadata(path)),
            _ => None,
        };

        // Re-extraction replaces this path's evidence wholesale.
        conn.execute("DELETE FROM evidence WHERE path_id = ?1", [id])
            .map_err(|e| e.to_string())?;

        if let Some(meta) = &meta {
            store_media_facts(conn, id, meta)?;
            if let Some(taken) = meta.taken {
                let raw = serde_json::to_string(&taken).map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT INTO evidence (path_id, source, raw, offset_known) \
                     VALUES (?1, 'metadata', ?2, ?3)",
                    params![
                        id,
                        raw,
                        matches!(taken, metadata::MetadataTimestamp::Absolute { .. }) as i64
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
        }

        if let Some(token) = timestamps::from_filename(&file_name) {
            let raw = serde_json::to_string(&token).map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO evidence (path_id, source, raw, offset_known) \
                 VALUES (?1, 'filename', ?2, ?3)",
                params![
                    id,
                    raw,
                    matches!(token, timestamps::FilenameTimestamp::EpochMillis(_)) as i64
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        conn.execute(
            "UPDATE paths SET indexed_at_utc = ?2 WHERE id = ?1",
            params![id, logging::now_iso_millis()],
        )
        .map_err(|e| e.to_string())?;
        stats.extracted += 1;
    }

    Ok(stats)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResolveScope {
    /// Rows never resolved (the normal pipeline tail).
    PendingOnly,
    /// Every non-missing extracted row — a settings change re-resolves the
    /// whole index from stored evidence, no file reads.
    All,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct ResolveStats {
    pub resolved: u64,
    pub undated: u64,
}

/// The pure resolution pass: stored evidence + stat columns → resolved
/// timestamp columns. Never opens a file.
pub fn resolve_from_evidence(
    conn: &Connection,
    config: &ResolutionConfig,
    scope: ResolveScope,
) -> Result<ResolveStats, String> {
    let mut stats = ResolveStats::default();

    let sql = match scope {
        ResolveScope::PendingOnly => {
            "SELECT id, mtime_ms, birthtime_ms FROM paths \
             WHERE missing = 0 AND indexed_at_utc IS NOT NULL AND resolved_source IS NULL"
        }
        ResolveScope::All => {
            "SELECT id, mtime_ms, birthtime_ms FROM paths \
             WHERE missing = 0 AND indexed_at_utc IS NOT NULL"
        }
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows: Vec<(i64, Option<i64>, Option<i64>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    for (id, mtime_ms, birthtime_ms) in rows {
        let mut meta_ts: Option<metadata::MetadataTimestamp> = None;
        let mut file_ts: Option<timestamps::FilenameTimestamp> = None;
        {
            let mut ev = conn
                .prepare("SELECT source, raw FROM evidence WHERE path_id = ?1")
                .map_err(|e| e.to_string())?;
            let found: Vec<(String, Option<String>)> = ev
                .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();
            for (source, raw) in found {
                let Some(raw) = raw else { continue };
                match source.as_str() {
                    "metadata" => meta_ts = serde_json::from_str(&raw).ok(),
                    "filename" => file_ts = serde_json::from_str(&raw).ok(),
                    _ => {}
                }
            }
        }

        match resolution::resolve(meta_ts, file_ts, mtime_ms, birthtime_ms, config) {
            Some(resolved) => {
                stats.resolved += 1;
                conn.execute(
                    "UPDATE paths SET resolved_utc_ms = ?2, resolved_source = ?3, \
                     date_only = ?4 WHERE id = ?1",
                    params![
                        id,
                        resolved.unix_ms,
                        resolved.source.as_str(),
                        resolved.date_only as i64
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            None => {
                stats.undated += 1;
                conn.execute(
                    "UPDATE paths SET resolved_utc_ms = NULL, resolved_source = 'undated', \
                     date_only = 0 WHERE id = ?1",
                    params![id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(stats)
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct PairStats {
    pub paired: u64,
}

/// Same-directory pairing: a companion attaches to the primary (image or
/// video) sharing its lowercased stem in the same directory — never across
/// directories, which is what makes rotating-name false links (GoPro)
/// structurally impossible. Deterministic: the lowest-id primary wins if
/// several share a stem. A companion with no primary stays unattached and
/// behaves as an other-file.
pub fn pair_companions(conn: &Connection) -> Result<PairStats, String> {
    // Unpair first. `companion_of` was previously assigned once and never
    // reconsidered, so moving a JPEG out of the folder in Finder — the app's
    // own supported out-of-app-changes path — left the RAW pointing at a row
    // marked missing. Every read model filters `companion_of IS NULL`, so that
    // RAW then appeared in NO section, no count and no issue row, and neither
    // a delete nor a move-out of the new JPEG picked it up. A full rescan did
    // not fix it, because nothing ever cleared the column.
    conn.execute(
        "UPDATE paths SET companion_of = NULL \
         WHERE companion_of IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM paths pri \
                           WHERE pri.id = paths.companion_of AND pri.missing = 0)",
        [],
    )
    .map_err(|e| e.to_string())?;
    let updated = conn
        .execute(
            "UPDATE paths SET companion_of = (
                SELECT p.id FROM paths p
                WHERE p.dir_path = paths.dir_path AND p.stem = paths.stem
                  AND p.kind IN ('image', 'video') AND p.missing = 0
                ORDER BY p.id LIMIT 1)
             WHERE kind = 'companion' AND missing = 0 AND companion_of IS NULL
               AND EXISTS (
                SELECT 1 FROM paths p
                WHERE p.dir_path = paths.dir_path AND p.stem = paths.stem
                  AND p.kind IN ('image', 'video') AND p.missing = 0)",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(PairStats {
        paired: updated as u64,
    })
}

fn store_content_hash(
    conn: &Connection,
    path_id: i64,
    hash: &str,
    size: i64,
    kind: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES (?1, ?2, ?3) \
         ON CONFLICT(hash) DO NOTHING",
        params![hash, size, kind],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE paths SET content_hash = ?2 WHERE id = ?1",
        params![path_id, hash],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn store_media_facts(
    conn: &Connection,
    path_id: i64,
    meta: &metadata::MediaMetadata,
) -> Result<(), String> {
    conn.execute(
        "UPDATE contents SET width = COALESCE(?2, width), height = COALESCE(?3, height), \
         duration_ms = COALESCE(?4, duration_ms), \
         camera_make = COALESCE(?5, camera_make), camera_model = COALESCE(?6, camera_model) \
         WHERE hash = (SELECT content_hash FROM paths WHERE id = ?1)",
        params![
            path_id,
            meta.width.map(i64::from),
            meta.height.map(i64::from),
            meta.duration_ms.map(|v| v as i64),
            meta.make,
            meta.model
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn record_issue(
    conn: &Connection,
    path: Option<String>,
    kind: &str,
    message: &str,
) -> Result<(), String> {
    // The issues table is the user-facing surface; the session log is the
    // debugging record — every recorded failure leaves a warn line too
    // (logging conventions' one-warn-per-failure rule for loops).
    logging::warn(
        "scan issue",
        serde_json::json!({ "kind": kind, "path": path, "detail": message }),
    );
    conn.execute(
        "INSERT INTO issues (path, kind, message, created_at_utc) VALUES (?1, ?2, ?3, ?4)",
        params![path, kind, message, logging::now_iso_millis()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn collect_rows_4(
    conn: &Connection,
    sql: &str,
) -> Result<Vec<(i64, String, String, String)>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn ensure_trailing_separator(path: &str) -> String {
    if path.ends_with(std::path::MAIN_SEPARATOR) {
        path.to_string()
    } else {
        format!("{path}{}", std::path::MAIN_SEPARATOR)
    }
}

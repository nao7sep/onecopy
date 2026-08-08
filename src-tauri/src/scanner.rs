//! The indexing core: walk → stat → hash tiers → metadata/evidence → resolve,
//! all checkpointed into the index DB so an interrupted run resumes where it
//! stopped (unchanged size+mtime rows are skipped on the next pass). Symlinks
//! are not followed; hard links are distinct paths by design.
//!
//! Hash tiering refinement over the plan's base rule: media files (images and
//! videos) are always fully hashed — preview generation reads every byte
//! anyway, and their cache/dedup identity is the hash — while other-files keep
//! the full tier: a unique size is never content-read (no duplicate can
//! exist), a size collision gets the prehash, and only prehash collisions get
//! the full hash. Same size + same prehash + different full hash among
//! supposed copies is recorded as a `copies-disagree` issue.
//!
//! This module is synchronous and testable against temp trees; the
//! thread-spawning, progress-event-emitting wrapper arrives with the Phase 3
//! UI.

use std::collections::HashMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection, OptionalExtension};

use crate::extensions;
use crate::hashing;
use crate::logging;
use crate::metadata;
use crate::resolution::{self, ResolutionConfig};
use crate::timestamps;

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
    pub thumb_edge: u32,
    pub preview_long_edge: u32,
    pub keep_awake: bool,
    pub cache_root: std::path::PathBuf,
}

pub fn settings_from_config(
    config: Option<&serde_json::Value>,
    data_root: &Path,
    now_ms: i64,
) -> ScanSettings {
    let defaults = crate::storage::DefaultConfig::default();
    let get = |key: &str| config.and_then(|c| c.get(key));

    let string_list = |key: &str, fallback: &[String]| -> Vec<String> {
        get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.as_str())
                    .map(|s| s.to_lowercase())
                    .collect()
            })
            .unwrap_or_else(|| fallback.to_vec())
    };
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

    ScanSettings {
        source_dirs: string_list_preserving_case(config, "sourceDirs"),
        lists: ScanLists {
            images: string_list("imageExtensions", &defaults.image_extensions),
            videos: string_list("videoExtensions", &defaults.video_extensions),
            companions: string_list("companionExtensions", &defaults.companion_extensions),
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
        },
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

    let hash_stats = hash_pending(conn)?;
    summary.full_hashed = hash_stats.full_hashed;
    summary.copies_disagree = hash_stats.copies_disagree;
    progress(
        "hash",
        format!(
            "{} hashed, {} unique skipped, {} disagreements",
            hash_stats.full_hashed, hash_stats.skipped_unique, hash_stats.copies_disagree
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
    let derive_stats = crate::preview::derive_images_pending(
        conn,
        &cache,
        settings.thumb_edge,
        settings.preview_long_edge,
    )?;
    summary.derived = derive_stats.derived;
    summary.derive_failed = derive_stats.failed;
    progress(
        "derive",
        format!("{} previews, {} failures", derive_stats.derived, derive_stats.failed),
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

    Ok(summary)
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct WalkStats {
    pub seen: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub marked_missing: u64,
}

/// Walks one source root: upserts every regular file as a `paths` row, skips
/// unchanged rows (same size + mtime — the checkpoint that makes rescans and
/// resumes cheap), resets content facts when a file changed, and marks rows
/// under the root that no longer exist as missing.
pub fn walk_root(conn: &Connection, root: &Path, lists: &ScanLists) -> Result<WalkStats, String> {
    let mut stats = WalkStats::default();
    let root_str = root.to_string_lossy().to_string();
    let scanned_at = logging::now_iso_millis();

    // Collect the currently-present set to diff against the DB afterwards.
    let mut present: Vec<String> = Vec::new();

    for entry in walkdir::WalkDir::new(root).follow_links(false) {
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
        let file_name = entry.file_name().to_string_lossy().to_string();
        // The app's own trash is never indexed.
        if abs.contains(".onecopy-trash") {
            continue;
        }

        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(err) => {
                record_issue(conn, Some(abs.clone()), "stat-error", &err.to_string())?;
                continue;
            }
        };
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
        let kind =
            extensions::classify(&ext, &lists.images, &lists.videos, &lists.companions).as_str();
        // Lowercased stem for the same-dir pairing rule (path comparison is
        // case-insensitive by fleet rule — macOS and Windows both are).
        let stem = Path::new(&file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&file_name)
            .to_lowercase();

        stats.seen += 1;
        present.push(abs.clone());

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
                // Unchanged: only clear a stale missing flag.
                conn.execute(
                    "UPDATE paths SET missing = 0 WHERE abs_path = ?1 AND missing = 1",
                    [&abs],
                )
                .map_err(|e| e.to_string())?;
                stats.unchanged += 1;
            }
            Some(_) => {
                // Changed on disk: reset the content facts; the hash and
                // resolve passes will redo them.
                conn.execute(
                    "UPDATE paths SET size = ?2, mtime_ms = ?3, birthtime_ms = ?4, ext = ?5, \
                     kind = ?6, stem = ?7, prehash = NULL, content_hash = NULL, \
                     indexed_at_utc = NULL, resolved_utc_ms = NULL, resolved_source = NULL, \
                     date_only = 0, missing = 0 WHERE abs_path = ?1",
                    params![abs, size, mtime_ms, birthtime_ms, ext, kind, stem],
                )
                .map_err(|e| e.to_string())?;
                stats.updated += 1;
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
                stats.added += 1;
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
    pub copies_disagree: u64,
    pub errors: u64,
}

/// The tiered content pass over rows without a content hash. Media rows always
/// get the full hash; other-file rows get it only when size and prehash both
/// collide. Contents rows are created per distinct hash; copies-disagree
/// anomalies are recorded as issues.
pub fn hash_pending(conn: &Connection) -> Result<HashStats, String> {
    let mut stats = HashStats::default();

    // --- Media: full hash unconditionally. ---
    let media: Vec<(i64, String, i64, String)> = collect_rows(
        conn,
        "SELECT id, abs_path, size, kind FROM paths \
         WHERE missing = 0 AND content_hash IS NULL AND kind IN ('image', 'video')",
    )?;
    for (id, abs, size, kind) in media {
        match hashing::full_hash(Path::new(&abs)) {
            Ok(hash) => {
                stats.full_hashed += 1;
                store_content_hash(conn, id, &hash, size, &kind)?;
            }
            Err(err) => {
                stats.errors += 1;
                record_issue(conn, Some(abs), "read-error", &err.to_string())?;
            }
        }
    }

    // --- Other files (companions included): the full tier. ---
    let others: Vec<(i64, String, i64, String)> = collect_rows(
        conn,
        "SELECT id, abs_path, size, kind FROM paths \
         WHERE missing = 0 AND content_hash IS NULL AND kind NOT IN ('image', 'video')",
    )?;

    // Group by size; unique sizes are never content-read.
    let mut by_size: HashMap<i64, Vec<(i64, String, String)>> = HashMap::new();
    for (id, abs, size, kind) in others {
        by_size.entry(size).or_default().push((id, abs, kind));
    }
    for (size, group) in by_size {
        if group.len() == 1 {
            stats.skipped_unique += 1;
            continue;
        }
        // Size collision: prehash each, then full-hash only prehash collisions.
        let mut by_prehash: HashMap<String, Vec<(i64, String, String)>> = HashMap::new();
        for (id, abs, kind) in group {
            match hashing::prehash(Path::new(&abs)) {
                Ok(pre) => {
                    stats.prehashed += 1;
                    conn.execute(
                        "UPDATE paths SET prehash = ?2 WHERE id = ?1",
                        params![id, pre],
                    )
                    .map_err(|e| e.to_string())?;
                    by_prehash.entry(pre).or_default().push((id, abs, kind));
                }
                Err(err) => {
                    stats.errors += 1;
                    record_issue(conn, Some(abs), "read-error", &err.to_string())?;
                }
            }
        }
        for (_pre, collided) in by_prehash {
            if collided.len() == 1 {
                stats.skipped_unique += 1;
                continue;
            }
            let group_len = collided.len();
            let mut hashes_in_group: Vec<String> = Vec::new();
            for (id, abs, kind) in collided {
                match hashing::full_hash(Path::new(&abs)) {
                    Ok(hash) => {
                        stats.full_hashed += 1;
                        if !hashes_in_group.contains(&hash) {
                            hashes_in_group.push(hash.clone());
                        }
                        store_content_hash(conn, id, &hash, size, &kind)?;
                    }
                    Err(err) => {
                        stats.errors += 1;
                        record_issue(conn, Some(abs), "read-error", &err.to_string())?;
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
    conn.execute(
        "INSERT INTO issues (path, kind, message, created_at_utc) VALUES (?1, ?2, ?3, ?4)",
        params![path, kind, message, logging::now_iso_millis()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn collect_rows(
    conn: &Connection,
    sql: &str,
) -> Result<Vec<(i64, String, i64, String)>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_store;

    fn lists() -> ScanLists {
        let owned = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        ScanLists {
            images: owned(extensions::IMAGE_EXTENSIONS),
            videos: owned(extensions::VIDEO_EXTENSIONS),
            companions: owned(extensions::COMPANION_EXTENSIONS),
        }
    }

    fn resolution_config() -> ResolutionConfig {
        ResolutionConfig {
            default_timezone: chrono_tz::Asia::Tokyo,
            good_range_start_year: 1995,
            now_ms: 1_786_492_800_000, // 2026-08-08T00:00:00Z
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        root: std::path::PathBuf,
        conn: Connection,
    }

    fn fixture(label: &str) -> Fixture {
        let dir = tempfile::Builder::new()
            .prefix(&format!("onecopy-scan-{label}-"))
            .tempdir()
            .unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        Fixture {
            _dir: dir,
            root,
            conn,
        }
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn walk_adds_then_skips_unchanged_then_marks_missing() {
        let f = fixture("walk");
        std::fs::write(f.root.join("IMG_20160305_123456.jpg"), b"aaa").unwrap();
        std::fs::write(f.root.join("notes.txt"), b"bbb").unwrap();

        let s1 = walk_root(&f.conn, &f.root, &lists()).unwrap();
        assert_eq!((s1.added, s1.unchanged, s1.marked_missing), (2, 0, 0));

        // Second pass: everything unchanged.
        let s2 = walk_root(&f.conn, &f.root, &lists()).unwrap();
        assert_eq!((s2.added, s2.unchanged), (0, 2));

        // Delete one file: the row is marked missing, never removed.
        std::fs::remove_file(f.root.join("notes.txt")).unwrap();
        let s3 = walk_root(&f.conn, &f.root, &lists()).unwrap();
        assert_eq!(s3.marked_missing, 1);
        assert_eq!(
            count(&f.conn, "SELECT COUNT(*) FROM paths WHERE missing = 1"),
            1
        );
    }

    #[test]
    fn media_is_always_fully_hashed_and_copies_collapse() {
        let f = fixture("media-hash");
        // Three identical copies in different subdirs, one distinct file.
        for sub in ["a", "b", "c"] {
            std::fs::create_dir_all(f.root.join(sub)).unwrap();
            std::fs::write(f.root.join(sub).join("x.jpg"), b"same-bytes").unwrap();
        }
        std::fs::write(f.root.join("unique.jpg"), b"different").unwrap();

        walk_root(&f.conn, &f.root, &lists()).unwrap();
        let stats = hash_pending(&f.conn).unwrap();
        assert_eq!(stats.full_hashed, 4);

        // One contents row for the three copies, one for the unique file.
        assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 2);
        let copies: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE content_hash = \
                 (SELECT content_hash FROM paths WHERE file_name = 'x.jpg' LIMIT 1)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(copies, 3);
    }

    #[test]
    fn other_files_with_unique_sizes_are_never_read() {
        let f = fixture("other-tier");
        std::fs::write(f.root.join("a.bin"), vec![1u8; 100]).unwrap();
        std::fs::write(f.root.join("b.bin"), vec![2u8; 200]).unwrap();

        walk_root(&f.conn, &f.root, &lists()).unwrap();
        let stats = hash_pending(&f.conn).unwrap();
        assert_eq!(stats.skipped_unique, 2);
        assert_eq!(stats.prehashed, 0);
        assert_eq!(stats.full_hashed, 0);
        assert_eq!(
            count(&f.conn, "SELECT COUNT(*) FROM paths WHERE content_hash IS NOT NULL"),
            0
        );
    }

    #[test]
    fn size_collisions_among_other_files_get_hashed_and_deduped() {
        let f = fixture("other-dup");
        std::fs::write(f.root.join("copy1.bin"), b"identical-data").unwrap();
        std::fs::write(f.root.join("copy2.bin"), b"identical-data").unwrap();

        walk_root(&f.conn, &f.root, &lists()).unwrap();
        let stats = hash_pending(&f.conn).unwrap();
        assert_eq!(stats.prehashed, 2);
        assert_eq!(stats.full_hashed, 2);
        assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 1);
    }

    #[test]
    fn diverged_copies_surface_as_a_copies_disagree_issue() {
        let f = fixture("disagree");
        // Same size, same 64K edges, different middle — the bit-rot shape.
        let mut a = vec![7u8; 200_000];
        let mut b = vec![7u8; 200_000];
        a[100_000] = 1;
        b[100_000] = 2;
        std::fs::write(f.root.join("rotted1.bin"), &a).unwrap();
        std::fs::write(f.root.join("rotted2.bin"), &b).unwrap();

        walk_root(&f.conn, &f.root, &lists()).unwrap();
        let stats = hash_pending(&f.conn).unwrap();
        assert_eq!(stats.copies_disagree, 1);
        assert_eq!(
            count(&f.conn, "SELECT COUNT(*) FROM issues WHERE kind = 'copies-disagree'"),
            1
        );
        // Both files keep their own distinct contents rows.
        assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM contents"), 2);
    }

    #[test]
    fn resolve_uses_filename_then_filesystem_and_flags_undated() {
        let f = fixture("resolve");
        // No EXIF in these bytes, so the filename is the winning source.
        std::fs::write(f.root.join("IMG_20160305_123456.jpg"), b"not-a-real-jpeg").unwrap();
        // No date anywhere in name or content: filesystem mtime wins.
        std::fs::write(f.root.join("scan.pdf"), b"pdf-ish").unwrap();

        walk_root(&f.conn, &f.root, &lists()).unwrap();
        hash_pending(&f.conn).unwrap();
        extract_pending(&f.conn).unwrap();
        let stats =
            resolve_from_evidence(&f.conn, &resolution_config(), ResolveScope::PendingOnly)
                .unwrap();
        assert_eq!(stats.resolved, 2);
        assert_eq!(stats.undated, 0);

        let (source, ms): (String, i64) = f
            .conn
            .query_row(
                "SELECT resolved_source, resolved_utc_ms FROM paths \
                 WHERE file_name = 'IMG_20160305_123456.jpg'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(source, "filename");
        // 2016-03-05 12:34:56 JST == 03:34:56 UTC.
        let expected = chrono::NaiveDate::from_ymd_opt(2016, 3, 5)
            .unwrap()
            .and_hms_opt(3, 34, 56)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(ms, expected);

        let source: String = f
            .conn
            .query_row(
                "SELECT resolved_source FROM paths WHERE file_name = 'scan.pdf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source, "filesystem");
    }

    #[test]
    fn companions_pair_same_directory_same_stem_only() {
        let f = fixture("pairing");
        let sub = f.root.join("gopro");
        std::fs::create_dir_all(&sub).unwrap();
        // RAW beside its JPEG (case differs — pairing is case-insensitive).
        std::fs::write(f.root.join("IMG_1234.JPG"), b"jpeg").unwrap();
        std::fs::write(f.root.join("img_1234.arw"), b"raw").unwrap();
        // THM beside its MP4.
        std::fs::write(sub.join("GOPR0001.MP4"), b"video").unwrap();
        std::fs::write(sub.join("GOPR0001.THM"), b"thumb").unwrap();
        // Same stem as the JPG but in another directory: must NOT pair.
        std::fs::write(sub.join("IMG_1234.arw"), b"stray raw").unwrap();

        walk_root(&f.conn, &f.root, &lists()).unwrap();
        let stats = pair_companions(&f.conn).unwrap();
        assert_eq!(stats.paired, 2);

        let paired_to_jpg: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM paths c JOIN paths p ON c.companion_of = p.id \
                 WHERE c.file_name = 'img_1234.arw' AND p.file_name = 'IMG_1234.JPG'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(paired_to_jpg, 1);

        let stray_unpaired: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE file_name = 'IMG_1234.arw' \
                 AND dir_path LIKE '%gopro' AND companion_of IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stray_unpaired, 1);

        // Idempotent: a second pass pairs nothing new.
        assert_eq!(pair_companions(&f.conn).unwrap().paired, 0);
    }

    #[test]
    fn settings_changes_re_resolve_from_evidence_without_file_reads() {
        let f = fixture("re-resolve");
        std::fs::write(f.root.join("IMG_20160305_123456.jpg"), b"not-a-real-jpeg").unwrap();
        walk_root(&f.conn, &f.root, &lists()).unwrap();
        hash_pending(&f.conn).unwrap();
        extract_pending(&f.conn).unwrap();
        resolve_from_evidence(&f.conn, &resolution_config(), ResolveScope::PendingOnly).unwrap();

        // Delete the file from disk: a re-resolve that needed to re-read it
        // would now fail or go undated. It must not — evidence is in the DB.
        std::fs::remove_file(f.root.join("IMG_20160305_123456.jpg")).unwrap();

        // Switch the default timezone JST → UTC and re-resolve everything.
        let utc_config = ResolutionConfig {
            default_timezone: chrono_tz::UTC,
            ..resolution_config()
        };
        let stats = resolve_from_evidence(&f.conn, &utc_config, ResolveScope::All).unwrap();
        assert_eq!(stats.resolved, 1);

        let ms: i64 = f
            .conn
            .query_row(
                "SELECT resolved_utc_ms FROM paths WHERE file_name = 'IMG_20160305_123456.jpg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // Under UTC the naive 12:34:56 now IS 12:34:56Z (was 03:34:56Z under JST).
        let expected = chrono::NaiveDate::from_ymd_opt(2016, 3, 5)
            .unwrap()
            .and_hms_opt(12, 34, 56)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(ms, expected);
    }

    #[test]
    fn app_trash_directories_are_never_indexed() {
        let f = fixture("trash-skip");
        let trash = f.root.join(".onecopy-trash").join("2026-08-08");
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(trash.join("deleted.jpg"), b"gone").unwrap();
        std::fs::write(f.root.join("kept.jpg"), b"here").unwrap();

        let stats = walk_root(&f.conn, &f.root, &lists()).unwrap();
        assert_eq!(stats.seen, 1);
        assert_eq!(count(&f.conn, "SELECT COUNT(*) FROM paths"), 1);
    }
}

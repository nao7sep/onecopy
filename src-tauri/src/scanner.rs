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
//! This module is synchronous and testable against temp trees. It owns the
//! typed progress facts because only the index pipeline knows what a durable
//! checkpoint means; `scan_runtime` owns worker lifetime and event transport.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    pub pairing_enabled: bool,
    pub keep_awake: bool,
    pub cache_root: std::path::PathBuf,
}

/// One honest snapshot of durable index work. Phase tokens are stable backend
/// facts; the frontend owns their words. `done/total` always describe a stable
/// unit chosen before the phase starts (sources for the filesystem walk, paths
/// for row phases, and fixed SQL steps for pairing). A streamed full hash adds
/// byte progress for the current file without changing the phase total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanPhase {
    Walk,
    Hash,
    Extract,
    Resolve,
    Pair,
    Indexed,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub phase: ScanPhase,
    pub done: u64,
    pub total: u64,
    pub current_path: Option<String>,
    pub discovered: Option<u64>,
    pub bytes_done: Option<u64>,
    pub bytes_total: Option<u64>,
    pub failures: u64,
    pub next_phase: Option<ScanPhase>,
}

impl ScanProgress {
    fn phase(phase: ScanPhase, total: u64, next_phase: Option<ScanPhase>) -> Self {
        Self {
            phase,
            done: 0,
            total,
            current_path: None,
            discovered: None,
            bytes_done: None,
            bytes_total: None,
            failures: 0,
            next_phase,
        }
    }

    fn at_path(
        phase: ScanPhase,
        done: u64,
        total: u64,
        path: &str,
        failures: u64,
        next_phase: ScanPhase,
    ) -> Self {
        let mut progress = Self::phase(phase, total, Some(next_phase));
        progress.done = done;
        progress.current_path = Some(crate::winpath::for_display(path).to_string());
        progress.failures = failures;
        progress
    }

    fn walk(done: u64, total: u64, root: &str, discovered: u64, failures: u64) -> Self {
        let mut progress = Self::at_path(
            ScanPhase::Walk,
            done,
            total,
            root,
            failures,
            ScanPhase::Hash,
        );
        progress.discovered = Some(discovered);
        progress
    }

    fn with_bytes(mut self, done: u64, total: u64) -> Self {
        self.bytes_done = Some(done);
        self.bytes_total = Some(total);
        self
    }

    fn completed(
        phase: ScanPhase,
        total: u64,
        failures: u64,
        next_phase: Option<ScanPhase>,
    ) -> Self {
        let mut progress = Self::phase(phase, total, next_phase);
        progress.done = total;
        progress.failures = failures;
        progress
    }
}

pub fn settings_from_config(
    config: Option<&serde_json::Value>,
    data_root: &Path,
    now_ms: i64,
) -> ScanSettings {
    let defaults = crate::storage::DefaultConfig::default();
    let get = |key: &str| config.and_then(|c| c.get(key));

    let tz: chrono_tz::Tz = get("defaultTimezone")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .or_else(|| defaults.default_timezone.parse().ok())
        .unwrap_or(chrono_tz::UTC);

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
        pairing_enabled: get("pairingEnabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(defaults.pairing_enabled),
        keep_awake: get("keepAwakeDuringIndexing")
            .and_then(|v| v.as_bool())
            .unwrap_or(defaults.keep_awake_during_indexing),
        cache_root: data_root.join(crate::storage::CACHE_DIR_NAME),
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
    pub failures: u64,
}

/// Escapes LIKE wildcards in a path prefix. `_` is common in real paths and
/// `!` appears in no sane one on either OS, so it is the escape character.
fn like_prefix(root: &str) -> String {
    let escaped = root
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_");
    format!("{}%", ensure_trailing_separator(&escaped))
}

/// Forgets every file under a root the user has stopped configuring.
///
/// Removing a source directory means "stop handling this folder". Without
/// this the rows simply stayed: an unconfigured root is never walked, so its
/// files were never marked missing either — they kept appearing in sections,
/// kept counting toward totals, and stayed deletable from a folder the app had
/// been told to leave alone. They are NOT marked missing, which would be a
/// false statement (the files are still on disk); they are dropped, because
/// the app is simply no longer their bookkeeper.
///
/// Content and cache follow the rule that already governs deletion: they go
/// only when no live path anywhere still points at them, so a photo that also
/// lives under a root the user KEPT is untouched — the duplicate case this
/// whole app is built around.
pub fn forget_unconfigured_roots(
    conn: &Connection,
    configured: &[String],
    cache: &crate::preview::CachePaths,
) -> Result<u64, String> {
    let mut stmt = conn
        .prepare("SELECT root FROM scan_dirs")
        .map_err(|e| e.to_string())?;
    let recorded: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    // Settle each configured root to the spelling the INDEX would use before
    // comparing. `scan_dirs` holds settled spellings, so comparing against the
    // raw config strings would read a mere canonicalization difference as "the
    // user removed this folder" and drop a whole drive's index.
    //
    // A root that cannot be resolved right now — an unplugged drive — counts as
    // STILL CONFIGURED. Being wrong in that direction leaves stale rows until
    // the drive returns; being wrong in the other direction destroys the index
    // for every file on it. Only one of those is recoverable.
    let mut keep: Vec<String> = Vec::new();
    for dir in configured {
        keep.push(dir.clone());
        // An absent Windows root cannot be canonicalized, but its ordinary
        // configured spelling still maps deterministically to the verbatim
        // spelling scan_dirs uses. Keep both so an unplugged drive is never
        // mistaken for a removed source and destructively forgotten.
        keep.push(
            crate::winpath::for_fs(Path::new(dir))
                .to_string_lossy()
                .to_string(),
        );
        if let Ok(settled) = settled_root(conn, Path::new(dir)) {
            keep.push(settled.to_string_lossy().to_string());
        }
    }
    let still_configured = |root: &str| {
        keep.iter()
            .any(|k| k == root || k.to_lowercase() == root.to_lowercase())
    };

    let mut forgotten = 0u64;
    for root in recorded {
        if still_configured(&root) {
            continue;
        }
        // Hashes that had a row here, so orphans can be collected after.
        let mut hashes_stmt = conn
            .prepare(
                "SELECT DISTINCT content_hash FROM paths \
                 WHERE abs_path LIKE ?1 ESCAPE '!' AND content_hash IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        let touched: Vec<String> = hashes_stmt
            .query_map([like_prefix(&root)], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        drop(hashes_stmt);

        // Companions first: their rows hold a foreign key to the primary.
        conn.execute(
            "DELETE FROM evidence WHERE path_id IN \
             (SELECT id FROM paths WHERE abs_path LIKE ?1 ESCAPE '!')",
            [like_prefix(&root)],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE paths SET companion_of = NULL WHERE abs_path LIKE ?1 ESCAPE '!'",
            [like_prefix(&root)],
        )
        .map_err(|e| e.to_string())?;
        let removed = conn
            .execute(
                "DELETE FROM paths WHERE abs_path LIKE ?1 ESCAPE '!'",
                [like_prefix(&root)],
            )
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM scan_dirs WHERE root = ?1", [&root])
            .map_err(|e| e.to_string())?;
        forgotten += removed as u64;

        for hash in touched {
            let live: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM paths WHERE content_hash = ?1 AND missing = 0",
                    [&hash],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if live == 0 {
                conn.execute("DELETE FROM paths WHERE content_hash = ?1", [&hash])
                    .map_err(|e| e.to_string())?;
                conn.execute(
                    "DELETE FROM similar_group_members WHERE content_hash = ?1",
                    [&hash],
                )
                .map_err(|e| e.to_string())?;
                conn.execute("DELETE FROM contents WHERE hash = ?1", [&hash])
                    .map_err(|e| e.to_string())?;
                crate::preview::remove_entries(cache, &hash);
            }
        }
        if removed > 0 {
            logging::info(
                "forgot an unconfigured source root",
                serde_json::json!({ "root": root, "rows": removed }),
            );
        }
    }
    Ok(forgotten)
}

/// Settles ONE spelling for a configured root, once per scan.
///
/// `paths.abs_path` is unique, so the same physical file reached under two
/// spellings becomes two rows — and the copy-count badge, which doubles as the
/// backup health check, then reports 2 for a file that exists once. Within a
/// single walk the spelling is consistent (every entry is joined onto the
/// root), so pinning the ROOT is enough to pin everything beneath it.
///
/// Two steps, because neither alone is sufficient:
///
/// 1. Canonicalize — resolves symlinks and makes the path absolute. On Windows
///    it also returns the disk's true casing and the long-path form. On macOS
///    it does NOT correct casing (verified: `realpath` echoes the spelling it
///    was given on a case-insensitive volume), which is why step 2 exists.
/// 2. Prefer a spelling already recorded in `scan_dirs` that differs only by
///    case. First-seen wins, so re-typing a configured path with different
///    capitalisation cannot fork the index. Matching case-insensitively is safe
///    HERE specifically: this compares roots, never individual files, and two
///    roots differing only by case cannot coexist on either target platform.
pub fn settled_root(conn: &Connection, configured: &Path) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(configured)
        .map_err(|e| format!("{}: {e}", configured.display()))?;
    let canonical_str = canonical.to_string_lossy().to_string();

    let mut stmt = conn
        .prepare("SELECT root FROM scan_dirs")
        .map_err(|e| e.to_string())?;
    let known: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    for root in known {
        if root != canonical_str && root.to_lowercase() == canonical_str.to_lowercase() {
            return Ok(PathBuf::from(root));
        }
    }
    Ok(canonical)
}

/// One full index run over every configured root: walk → hash → extract →
/// resolve → pair, reporting typed durable progress. Derived media
/// is deliberately owned by `derived_work`, which wakes after this returns.
pub fn run_full_scan(
    conn: &Connection,
    settings: &ScanSettings,
    progress: &dyn Fn(ScanProgress),
) -> Result<ScanSummary, String> {
    let mut summary = ScanSummary::default();

    // Reconcile configuration BEFORE walking: a root the user removed should
    // stop appearing the moment the next scan runs, not linger until someone
    // notices its files in a section.
    let cache = crate::preview::CachePaths::new(settings.cache_root.clone());
    forget_unconfigured_roots(conn, &settings.source_dirs, &cache)?;

    let root_total = settings.source_dirs.len() as u64;
    let mut walk_failures = 0u64;
    progress(ScanProgress::phase(
        ScanPhase::Walk,
        root_total,
        Some(ScanPhase::Hash),
    ));
    for (root_index, root) in settings.source_dirs.iter().enumerate() {
        // One settled spelling per root, so a re-typed capitalisation cannot
        // index the same files a second time.
        let root = settled_root(conn, Path::new(root))?;
        let root = root.to_string_lossy().to_string();
        let root = root.as_str();
        let stats = walk_root_with_progress(
            conn,
            Path::new(root),
            &settings.lists,
            root_index as u64,
            root_total,
            walk_failures,
            progress,
        )?;
        summary.roots += 1;
        summary.seen += stats.seen;
        summary.added += stats.added;
        walk_failures += stats.errors;
        summary.failures += stats.errors;
    }

    run_index_tail(conn, settings, progress, &mut summary)?;
    Ok(summary)
}

/// Whether any configured root still owes a full walk — it has never been
/// walked to completion, or a walk over it was interrupted. This is the one
/// thing `pending_index_work_exists` cannot see: its probes are row-level, so
/// once the tail drains the rows a partial walk created, it reports clean
/// forever while whole directories remain unread.
pub fn walk_owed(conn: &Connection, roots: &[String]) -> Result<bool, String> {
    for root in roots {
        // `walk_root` checkpoints the settled root, not the literal configured
        // spelling. Resolve through that same authority so case and symlink aliases
        // find the completed row; the config string itself remains untouched.
        let settled = settled_root(conn, Path::new(root))?;
        let fs_root = crate::winpath::for_fs(&settled);
        let complete: Option<bool> = conn
            .query_row(
                "SELECT last_completed_at_utc IS NOT NULL AND dirty = 0 \
                 FROM scan_dirs WHERE root = ?1",
                params![fs_root.to_string_lossy().as_ref()],
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

pub fn pending_index_work_exists(conn: &Connection) -> Result<bool, String> {
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
        "SELECT EXISTS(SELECT 1 FROM paths \
         WHERE missing = 0 AND indexed_at_utc IS NULL)",
    )? {
        return Ok(true);
    }
    if probe(
        "SELECT EXISTS(SELECT 1 FROM paths WHERE missing = 0 \
         AND indexed_at_utc IS NOT NULL AND resolved_source IS NULL)",
    )? {
        return Ok(true);
    }
    // A build that learns a new metadata fact must backfill an already-complete
    // pre-release index once. The NULL raw row is still evidence: it means the
    // file was checked and carries no Live Photo identifier.
    if probe(
        "SELECT EXISTS(SELECT 1 FROM paths p \
         WHERE p.missing = 0 AND p.kind IN ('image', 'video') \
           AND NOT EXISTS (SELECT 1 FROM evidence e \
                           WHERE e.path_id = p.id \
                             AND e.source = 'live-photo-identifier'))",
    )? {
        return Ok(true);
    }
    Ok(false)
}

/// The index pipeline minus the walk: hash → extract → resolve → pair, over
/// whatever the checkpoints left pending. Shared by the full scan, startup
/// resume, watcher, and scoped section rescan.
pub fn run_index_tail(
    conn: &Connection,
    settings: &ScanSettings,
    progress: &dyn Fn(ScanProgress),
    summary: &mut ScanSummary,
) -> Result<(), String> {
    let cache = crate::preview::CachePaths::new(settings.cache_root.clone());
    let hash_stats = hash_pending_with_progress(conn, &cache, progress)?;
    summary.full_hashed = hash_stats.full_hashed;
    summary.copies_disagree = hash_stats.copies_disagree;
    summary.failures += hash_stats.errors;

    extract_pending_with_progress(conn, progress)?;

    let resolve_stats = resolve_from_evidence_with_progress(
        conn,
        &settings.resolution,
        ResolveScope::PendingOnly,
        progress,
    )?;
    summary.resolved = resolve_stats.resolved;
    summary.undated = resolve_stats.undated;

    let pair_stats = pair_companions_with_progress(conn, settings.pairing_enabled, progress)?;
    summary.paired = pair_stats.paired;

    progress(ScanProgress::completed(
        ScanPhase::Indexed,
        1,
        summary.failures,
        None,
    ));

    Ok(())
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct WalkStats {
    pub seen: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub marked_missing: u64,
    pub errors: u64,
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
    let meta = std::fs::metadata(crate::winpath::for_fs(path).as_ref()).map_err(|e| e.to_string())?;
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
    walk_root_with_progress(conn, root, lists, 0, 1, 0, &|_| {})
}

fn walk_root_with_progress(
    conn: &Connection,
    root: &Path,
    lists: &ScanLists,
    completed_roots: u64,
    total_roots: u64,
    failures_before: u64,
    progress: &dyn Fn(ScanProgress),
) -> Result<WalkStats, String> {
    let mut stats = WalkStats::default();
    // Pick the filesystem spelling once. On Windows WalkDir inherits the
    // `\\?\` form into every entry it yields, so scan_dirs and the missing-row
    // prefix must use that same spelling; mixing an ordinary root with
    // verbatim child rows makes vanished files remain falsely live forever.
    let fs_root = crate::winpath::for_fs(root);
    let root_str = fs_root.to_string_lossy().to_string();
    let scanned_at = logging::now_iso_millis();

    progress(ScanProgress::walk(
        completed_roots,
        total_roots,
        &root_str,
        0,
        failures_before,
    ));

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
    // One probe up front so a clean index never pays a per-file DELETE.
    let issues_present = crate::index_store::any_issues(conn);

    // The walk root carries the long-path form so every entry beneath it
    // inherits it; without this a deep tree is simply invisible on Windows.
    for entry in walkdir::WalkDir::new(fs_root.as_ref()).follow_links(false) {
        check_cancel()?;
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                stats.errors += 1;
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
        if abs.contains(crate::trash::TRASH_DIR_NAME) {
            continue;
        }

        stats.seen += 1;
        present.push(abs.clone());

        match upsert_file(conn, path, lists) {
            Ok(outcome) => {
                match outcome {
                    Upsert::Added => stats.added += 1,
                    Upsert::Updated => stats.updated += 1,
                    Upsert::Unchanged => stats.unchanged += 1,
                }
                // The success counterpart: a re-walked file that now stats
                // clean drops its scan-condition rows (current-state issues).
                if issues_present {
                    crate::index_store::clear_issues(conn, &abs, &["stat-error", "walk-error"])?;
                }
            }
            Err(err) => {
                stats.seen -= 1;
                present.pop();
                stats.errors += 1;
                record_issue(conn, Some(abs), "stat-error", &err)?;
            }
        }

        progress(ScanProgress::walk(
            completed_roots,
            total_roots,
            &root_str,
            stats.seen,
            failures_before + stats.errors,
        ));
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

    progress(ScanProgress::walk(
        completed_roots + 1,
        total_roots,
        &root_str,
        stats.seen,
        failures_before + stats.errors,
    ));

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
    hash_pending_with_progress(conn, cache, &|_| {})
}

fn hash_pending_with_progress(
    conn: &Connection,
    cache: &crate::preview::CachePaths,
    progress: &dyn Fn(ScanProgress),
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
    let total = rows.len() as u64;
    let mut done = 0u64;
    progress(ScanProgress::phase(
        ScanPhase::Hash,
        total,
        Some(ScanPhase::Extract),
    ));

    let report_path =
        |row: &Row, done: u64, failures: u64, bytes_done: Option<u64>, bytes_total: Option<u64>| {
            let snapshot = ScanProgress::at_path(
                ScanPhase::Hash,
                done,
                total,
                &row.abs,
                failures,
                ScanPhase::Extract,
            );
            progress(match (bytes_done, bytes_total) {
                (Some(done), Some(total)) => snapshot.with_bytes(done, total),
                _ => snapshot,
            });
        };

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
    let issues_present = crate::index_store::any_issues(conn);
    let land_full_hash =
        |row: &Row, stats: &mut HashStats, done_before: u64| -> Result<Option<String>, String> {
        let byte_progress = |bytes_done: u64, bytes_total: u64| {
            report_path(
                row,
                done_before,
                stats.errors,
                Some(bytes_done),
                Some(bytes_total),
            );
        };
        match hashing::full_hash_cancellable_with_progress(
            Path::new(&row.abs),
            &SCAN_CANCEL,
            &byte_progress,
        ) {
            Ok(hash) => {
                stats.full_hashed += 1;
                // A read that succeeds proves the earlier failure resolved —
                // and this cohort's hashes now speak for copies-disagree, so
                // its stale row goes too (re-recorded below if still true).
                if issues_present {
                    crate::index_store::clear_issues(
                        conn,
                        &row.abs,
                        &["read-error", "copies-disagree"],
                    )?;
                }
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
            report_path(&group[0], done, stats.errors, None, None);
            settle_unique(&group[0], &mut stats)?;
            done += 1;
            report_path(&group[0], done, stats.errors, None, None);
            continue;
        }
        if known_sizes.contains(&size) {
            // Collides with established content: the prehash tier cannot
            // decide (established media were never prehashed) — read fully.
            for row in &group {
                check_cancel()?;
                let _ = land_full_hash(row, &mut stats, done)?;
                done += 1;
                report_path(row, done, stats.errors, None, None);
            }
            continue;
        }
        // Size collision within the pending set: prehash each, then
        // full-hash only prehash collisions.
        let mut by_prehash: HashMap<String, Vec<Row>> = HashMap::new();
        for mut row in group {
            check_cancel()?;
            report_path(&row, done, stats.errors, None, None);
            let pre = match &row.prehash {
                Some(pre) => Some(pre.clone()),
                None => match hashing::prehash(Path::new(&row.abs)) {
                    Ok(pre) => {
                        stats.prehashed += 1;
                        if issues_present {
                            crate::index_store::clear_issues(conn, &row.abs, &["read-error"])?;
                        }
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
                        done += 1;
                        report_path(&row, done, stats.errors, None, None);
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
                done += 1;
                report_path(&collided[0], done, stats.errors, None, None);
                continue;
            }
            let group_len = collided.len();
            let mut hashes_in_group: Vec<String> = Vec::new();
            for row in &collided {
                check_cancel()?;
                if let Some(hash) = land_full_hash(row, &mut stats, done)? {
                    if !hashes_in_group.contains(&hash) {
                        hashes_in_group.push(hash);
                    }
                }
                done += 1;
                report_path(row, done, stats.errors, None, None);
            }
            // Same size + same prehash + diverging full hashes: bit rot or a
            // divergent sync among supposed copies — surface it.
            if hashes_in_group.len() > 1 {
                stats.copies_disagree += 1;
                // One row PER FILE: (kind, path) identity needs a real anchor,
                // and naming the disagreeing files is what lets the user act.
                for row in &collided {
                    record_issue(
                        conn,
                        Some(row.abs.clone()),
                        "copies-disagree",
                        &format!(
                            "{group_len} same-size same-prehash files split into {} distinct contents (size {size}) — bit rot or a divergent sync among supposed copies",
                            hashes_in_group.len()
                        ),
                    )?;
                }
            }
        }
    }

    progress(ScanProgress::completed(
        ScanPhase::Hash,
        total,
        stats.errors,
        Some(ScanPhase::Extract),
    ));

    Ok(stats)
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct ExtractStats {
    pub extracted: u64,
}

pub const LIVE_PHOTO_REPAIR_PAGE_SIZE: usize = 64;

/// The evidence pass: reads in-file metadata (per kind) and runs the filename
/// tokenizer for rows not yet extracted, persisting each finding as a
/// serialized evidence row. This is the ONLY place resolution inputs touch a
/// file; after it, timezone/good-range/pattern changes re-resolve purely from
/// the DB.
pub fn extract_pending(conn: &Connection) -> Result<ExtractStats, String> {
    extract_pending_with_progress(conn, &|_| {})
}

fn extract_pending_with_progress(
    conn: &Connection,
    progress: &dyn Fn(ScanProgress),
) -> Result<ExtractStats, String> {
    let mut stats = ExtractStats::default();

    let rows: Vec<(i64, String, String, String)> = collect_rows_4(
        conn,
        "SELECT id, abs_path, file_name, kind FROM paths \
         WHERE missing = 0 AND indexed_at_utc IS NULL",
    )?;
    let repair_total = conn
        .query_row(
            "SELECT COUNT(*) FROM paths p \
             WHERE p.missing = 0 AND p.indexed_at_utc IS NOT NULL \
               AND p.kind IN ('image', 'video') \
               AND NOT EXISTS (SELECT 1 FROM evidence e \
                               WHERE e.path_id = p.id \
                                 AND e.source = 'live-photo-identifier')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())? as u64;
    let total = rows.len() as u64 + repair_total;
    let mut done = 0u64;
    progress(ScanProgress::phase(
        ScanPhase::Extract,
        total,
        Some(ScanPhase::Resolve),
    ));

    for (id, abs, file_name, kind) in rows {
        check_cancel()?;
        progress(ScanProgress::at_path(
            ScanPhase::Extract,
            done,
            total,
            &abs,
            0,
            ScanPhase::Resolve,
        ));
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
            if kind == "image" || kind == "video" {
                conn.execute(
                    "INSERT INTO evidence (path_id, source, raw, offset_known) \
                     VALUES (?1, 'live-photo-identifier', ?2, 0)",
                    params![id, meta.live_photo_identifier.as_deref()],
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
        done += 1;
        progress(ScanProgress::at_path(
            ScanPhase::Extract,
            done,
            total,
            &abs,
            0,
            ScanPhase::Resolve,
        ));
    }

    // Existing pre-release indexes already have `indexed_at_utc` checkpoints
    // but no Live Photo evidence. Backfill exactly those media rows once into
    // the existing evidence store; a NULL raw value is the durable "checked,
    // absent" result, so later scans do not reopen every family photo.
    let mut after_id = 0;
    loop {
        let pending_live_photo = live_photo_repair_candidates(
            conn,
            after_id,
            LIVE_PHOTO_REPAIR_PAGE_SIZE,
        )?;
        if pending_live_photo.is_empty() {
            break;
        }
        for (id, abs, kind) in pending_live_photo {
            check_cancel()?;
            progress(ScanProgress::at_path(
                ScanPhase::Extract,
                done,
                total,
                &abs,
                0,
                ScanPhase::Resolve,
            ));
            let path = Path::new(&abs);
            let identifier = match kind.as_str() {
                "image" => crate::live_photo::still_content_identifier(path),
                "video" => crate::live_photo::quicktime_content_identifier(path),
                _ => None,
            };
            conn.execute(
                "INSERT INTO evidence (path_id, source, raw, offset_known) \
                 VALUES (?1, 'live-photo-identifier', ?2, 0)",
                params![id, identifier],
            )
            .map_err(|e| e.to_string())?;
            stats.extracted += 1;
            done += 1;
            after_id = id;
            progress(ScanProgress::at_path(
                ScanPhase::Extract,
                done,
                total,
                &abs,
                0,
                ScanPhase::Resolve,
            ));
        }
    }

    progress(ScanProgress::completed(
        ScanPhase::Extract,
        total,
        0,
        Some(ScanPhase::Resolve),
    ));

    Ok(stats)
}

/// One stable page of legacy media rows missing the durable Live Photo
/// evidence receipt. Each completed page disappears from this query, so the
/// repair resumes naturally after cancellation without an in-memory ledger.
pub fn live_photo_repair_candidates(
    conn: &Connection,
    after_id: i64,
    limit: usize,
) -> Result<Vec<(i64, String, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.abs_path, p.kind FROM paths p \
             WHERE p.missing = 0 AND p.id > ?1 \
               AND p.kind IN ('image', 'video') \
               AND NOT EXISTS (SELECT 1 FROM evidence e \
                               WHERE e.path_id = p.id \
                                 AND e.source = 'live-photo-identifier') \
             ORDER BY p.id LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![after_id, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
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

const RESOLVE_PAGE_SIZE: usize = 256;

/// The pure resolution pass: stored evidence + stat columns → resolved
/// timestamp columns. Never opens a file.
pub fn resolve_from_evidence(
    conn: &Connection,
    config: &ResolutionConfig,
    scope: ResolveScope,
) -> Result<ResolveStats, String> {
    resolve_from_evidence_with_progress(conn, config, scope, &|_| {})
}

fn resolve_from_evidence_with_progress(
    conn: &Connection,
    config: &ResolutionConfig,
    scope: ResolveScope,
    progress: &dyn Fn(ScanProgress),
) -> Result<ResolveStats, String> {
    let mut stats = ResolveStats::default();

    let predicate = match scope {
        ResolveScope::PendingOnly => {
            "missing = 0 AND indexed_at_utc IS NOT NULL AND resolved_source IS NULL"
        }
        ResolveScope::All => {
            "missing = 0 AND indexed_at_utc IS NOT NULL"
        }
    };
    let total = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM paths WHERE {predicate}"),
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())? as u64;
    let mut done = 0u64;
    let mut after_id = 0i64;
    progress(ScanProgress::phase(
        ScanPhase::Resolve,
        total,
        Some(ScanPhase::Pair),
    ));
    let page_sql = format!(
        "SELECT id, abs_path, mtime_ms, birthtime_ms FROM paths \
         WHERE {predicate} AND id > ?1 ORDER BY id LIMIT ?2"
    );
    let mut page_stmt = conn.prepare(&page_sql).map_err(|e| e.to_string())?;
    let mut evidence_stmt = conn
        .prepare("SELECT source, raw FROM evidence WHERE path_id = ?1")
        .map_err(|e| e.to_string())?;
    let mut resolved_stmt = conn
        .prepare(
            "UPDATE paths SET resolved_utc_ms = ?2, resolved_source = ?3, \
             date_only = ?4 WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let mut undated_stmt = conn
        .prepare(
            "UPDATE paths SET resolved_utc_ms = NULL, resolved_source = 'undated', \
             date_only = 0 WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;

    loop {
        let rows: Vec<(i64, String, Option<i64>, Option<i64>)> = page_stmt
            .query_map(params![after_id, RESOLVE_PAGE_SIZE as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        if rows.is_empty() {
            break;
        }

        for (id, abs, mtime_ms, birthtime_ms) in rows {
            check_cancel()?;
            progress(ScanProgress::at_path(
                ScanPhase::Resolve,
                done,
                total,
                &abs,
                0,
                ScanPhase::Pair,
            ));
            let mut meta_ts: Option<metadata::MetadataTimestamp> = None;
            let mut file_ts: Option<timestamps::FilenameTimestamp> = None;
            {
                let found: Vec<(String, Option<String>)> = evidence_stmt
                    .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map_err(|e| e.to_string())?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| e.to_string())?;
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
                    resolved_stmt
                        .execute(params![
                            id,
                            resolved.unix_ms,
                            resolved.source.as_str(),
                            resolved.date_only as i64
                        ])
                        .map_err(|e| e.to_string())?;
                }
                None => {
                    stats.undated += 1;
                    undated_stmt
                        .execute(params![id])
                        .map_err(|e| e.to_string())?;
                }
            }
            after_id = id;
            done += 1;
            progress(ScanProgress::at_path(
                ScanPhase::Resolve,
                done,
                total,
                &abs,
                0,
                ScanPhase::Pair,
            ));
        }
    }

    progress(ScanProgress::completed(
        ScanPhase::Resolve,
        total,
        0,
        Some(ScanPhase::Pair),
    ));

    Ok(stats)
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct PairStats {
    pub paired: u64,
}

/// Rebuilds every enabled companion relationship. RAW/sidecar companions use
/// same-directory + lowercased stem. Live Photo MOVs use same-directory +
/// exact Apple content identifier and may have unrelated stems. The lowest-id
/// primary wins any ambiguous match. Disabled pairing leaves every row
/// independent.
pub fn pair_companions(conn: &Connection, enabled: bool) -> Result<PairStats, String> {
    pair_companions_with_progress(conn, enabled, &|_| {})
}

fn pair_companions_with_progress(
    conn: &Connection,
    enabled: bool,
    progress: &dyn Fn(ScanProgress),
) -> Result<PairStats, String> {
    let total = if enabled { 3 } else { 1 };
    let mut phase = ScanProgress::phase(
        ScanPhase::Pair,
        total,
        Some(ScanPhase::Indexed),
    );
    progress(phase.clone());
    conn.execute("UPDATE paths SET companion_of = NULL WHERE companion_of IS NOT NULL", [])
        .map_err(|e| e.to_string())?;
    phase.done = 1;
    progress(phase.clone());
    if !enabled {
        return Ok(PairStats::default());
    }

    check_cancel()?;
    let mut updated = conn
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
    phase.done = 2;
    progress(phase.clone());

    check_cancel()?;
    updated += conn
        .execute(
            "UPDATE paths SET companion_of = (
               SELECT image.id
               FROM evidence video_id
               JOIN evidence image_id
                 ON image_id.source = 'live-photo-identifier'
                AND image_id.raw = video_id.raw
               JOIN paths image ON image.id = image_id.path_id
               WHERE video_id.path_id = paths.id
                 AND video_id.source = 'live-photo-identifier'
                 AND video_id.raw IS NOT NULL
                 AND image.kind = 'image' AND image.missing = 0
                 AND image.dir_path = paths.dir_path
               ORDER BY image.id LIMIT 1)
             WHERE kind = 'video' AND missing = 0 AND companion_of IS NULL
               AND EXISTS (
                 SELECT 1
                 FROM evidence video_id
                 JOIN evidence image_id
                   ON image_id.source = 'live-photo-identifier'
                  AND image_id.raw = video_id.raw
                 JOIN paths image ON image.id = image_id.path_id
                 WHERE video_id.path_id = paths.id
                   AND video_id.source = 'live-photo-identifier'
                   AND video_id.raw IS NOT NULL
                   AND image.kind = 'image' AND image.missing = 0
                   AND image.dir_path = paths.dir_path)",
            [],
        )
        .map_err(|e| e.to_string())?;
    phase.done = 3;
    progress(phase);
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
    crate::index_store::upsert_issue(conn, path.as_deref(), kind, message)
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

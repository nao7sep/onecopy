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
pub const WALK_ERROR: &str = "walk-error";
pub const STAT_ERROR: &str = "stat-error";
pub const READ_ERROR: &str = "read-error";
pub const METADATA_READ_ERROR: &str = "metadata-read-error";
pub const COPIES_DISAGREE: &str = "copies-disagree";
const PATH_SCAN_ISSUES: &[&str] = &[WALK_ERROR, STAT_ERROR, READ_ERROR, METADATA_READ_ERROR, COPIES_DISAGREE];

/// The catalogue key for a scan-time Issue's kind: OneCopy's own sentence
/// follows the interface language, while whatever WalkDir or the OS reported
/// stays in `message`, as recorded, after it (R5.5 D-L12, D-L13). The one
/// place that resolves this rule for every scanner- and watcher-raised path
/// Issue.
pub(crate) fn scan_issue_message_key(kind: &str) -> &'static str {
    match kind {
        STAT_ERROR => "notice.sourceFileFailed",
        COPIES_DISAGREE => "notice.copiesDisagree",
        _ => "notice.sourceEntryFailed",
    }
}

pub(crate) fn mark_path_missing(conn: &Connection, path: &str) -> Result<(), String> {
    conn.execute("UPDATE paths SET missing = 1 WHERE abs_path = ?1", [path])
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(conn, path, PATH_SCAN_ISSUES).map(|_| ())
}

/// Closes the path-scan Issues of every path `path_predicate` (SQL over
/// `path`, its parameters numbered from `?2`) selects.
fn close_path_scan_issues(
    conn: &Connection,
    path_predicate: &str,
    path_params: &[&dyn rusqlite::ToSql],
) -> Result<(), String> {
    for kind in PATH_SCAN_ISSUES {
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![kind];
        values.extend_from_slice(path_params);
        crate::index_store::close_issues(
            conn,
            "resolved",
            &format!("kind = ?1 AND {path_predicate}"),
            &values,
        )?;
    }
    Ok(())
}

/// Marks every known row under `dir` (recursive prefix match) missing — used
/// when a watcher directory has vanished as a whole, so a per-file `read_dir`
/// diff is impossible. Returns the number of rows newly marked. Pages through
/// the projection-batch publisher like the settings re-resolve, so a large
/// vanished folder never holds the write lock for its whole size.
pub(crate) fn mark_missing_under(conn: &Connection, dir: &str) -> Result<u64, String> {
    let prefix = like_prefix(dir);
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS missing_under_page (id INTEGER PRIMARY KEY);",
    )
    .map_err(|error| error.to_string())?;
    let mut changed = 0u64;
    loop {
        let mut marked = 0u64;
        crate::index_store::publish_paths_batch(
            conn,
            |tx| {
                tx.execute("DELETE FROM missing_under_page", [])
                    .map_err(|error| error.to_string())?;
                tx.execute(
                    "INSERT INTO missing_under_page SELECT id FROM paths \
                     WHERE abs_path LIKE ?1 ESCAPE '!' AND missing = 0 LIMIT ?2",
                    params![prefix, RESOLVE_PAGE_SIZE as i64],
                )
                .map_err(|error| error.to_string())?;
                tx.execute(
                    "INSERT OR IGNORE INTO batch_touched_hashes SELECT content_hash FROM paths \
                     WHERE content_hash IS NOT NULL AND id IN (SELECT id FROM missing_under_page)",
                    [],
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
            },
            |tx| {
                marked = tx
                    .execute(
                        "UPDATE paths SET missing = 1 WHERE id IN (SELECT id FROM missing_under_page)",
                        [],
                    )
                    .map_err(|error| error.to_string())? as u64;
                Ok(())
            },
        )?;
        changed += marked;
        if marked < RESOLVE_PAGE_SIZE as u64 {
            break;
        }
        // Let a concurrent writer in between pages (see `re_resolve_all_with_progress`).
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    close_path_scan_issues(conn, "path LIKE ?2 ESCAPE '!'", &[&prefix])?;
    Ok(changed)
}

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
    pub audio: Vec<String>,
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
    pub cache_root: std::path::PathBuf,
}

impl ScanSettings {
    /// The app's data root, derived from `cache_root` (always
    /// `<data root>/cache`) rather than re-plumbed everywhere `ScanSettings`
    /// already travels. Used to exclude the app's own storage from discovery
    /// (R6-02).
    pub fn data_root(&self) -> &Path {
        self.cache_root.parent().unwrap_or(&self.cache_root)
    }
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
            audio: owned(extensions::AUDIO_EXTENSIONS),
            companions: owned(extensions::COMPANION_EXTENSIONS),
        },
        resolution: ResolutionConfig {
            default_timezone: tz,
            good_range_start_year: 1995,
            now_ms,
        },
        pairing_enabled: true,
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

fn relationship_path_key(path: &str) -> String {
    let mut key = path.replace('\\', "/").to_lowercase();
    while key.len() > 1 && key.ends_with('/') {
        key.pop();
    }
    key
}

pub(crate) fn directory_belongs_to_root(directory: &str, root: &str) -> bool {
    let directory = relationship_path_key(directory);
    let root = relationship_path_key(root);
    (root == "/" && directory.starts_with('/'))
        || directory == root
        || (directory.starts_with(&root)
            && directory
                .as_bytes()
                .get(root.len())
                .is_some_and(|separator| *separator == b'/'))
}

/// Marks roots whose companion projection needs repair. This receipt is
/// separate from `dirty`, which means a source walk itself was interrupted;
/// completing relationships must never claim that unread directories were
/// walked. Roots already carrying relationship debt are not returned, so a
/// scoped caller cannot clear debt it did not create.
pub fn begin_scoped_index_repair(
    conn: &Connection,
    dirs: &[String],
) -> Result<Vec<String>, String> {
    if dirs.is_empty() {
        return Ok(Vec::new());
    }
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
    let mut statement = transaction
        .prepare(
            "SELECT root FROM scan_dirs
             WHERE last_completed_at_utc IS NOT NULL AND relationship_dirty = 0",
        )
        .map_err(|error| error.to_string())?;
    let clean_roots = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    drop(statement);

    let mut touched = std::collections::HashSet::<String>::new();
    for dir in dirs {
        let mut matched = false;
        for root in &clean_roots {
            if directory_belongs_to_root(dir, root) {
                touched.insert(root.clone());
                matched = true;
            }
        }
        // An alias or vanished directory may not compare lexically. Marking
        // every clean root is conservative but recoverable; failing to mark
        // any would let a crash preserve stale relationships indefinitely.
        if !matched {
            touched.extend(clean_roots.iter().cloned());
        }
    }

    let mut roots: Vec<String> = touched.into_iter().collect();
    roots.sort_by_key(|root| root.to_lowercase());
    for root in &roots {
        transaction
            .execute(
                "UPDATE scan_dirs SET relationship_dirty = 1
             WHERE root = ?1 AND last_completed_at_utc IS NOT NULL
               AND relationship_dirty = 0",
                [root],
            )
            .map_err(|error| error.to_string())?;
    }
    crate::records::commit(transaction).map_err(|error| error.to_string())?;
    Ok(roots)
}

/// Runs `work` under the scoped repair receipt for `dirs`: the roots it marks
/// are cleared only when `work` succeeds, so a failed or cancelled repair is
/// retried by a later index repair.
pub fn with_scoped_index_repair<T>(
    conn: &Connection,
    dirs: &[String],
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let repair_roots = begin_scoped_index_repair(conn, dirs)?;
    let value = work()?;
    complete_scoped_index_repair(conn, &repair_roots)?;
    Ok(value)
}

pub fn complete_scoped_index_repair(
    conn: &Connection,
    roots_marked_by_repair: &[String],
) -> Result<(), String> {
    for root in roots_marked_by_repair {
        conn.execute(
            "UPDATE scan_dirs SET relationship_dirty = 0 WHERE root = ?1",
            [root],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
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
        .prepare("SELECT root, configured_root FROM scan_dirs")
        .map_err(|e| e.to_string())?;
    let recorded: Vec<(String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);

    // Settle each configured root to the spelling the INDEX would use before
    // comparing. `scan_dirs` holds settled spellings, so comparing against the
    // raw config strings would read a mere canonicalization difference as "the
    // user removed this folder" and drop a whole drive's index.
    //
    // A root that cannot be resolved right now — an unplugged drive — counts as
    // STILL CONFIGURED. Being wrong in that direction leaves stale rows until
    // the drive returns; being wrong in the other direction destroys the index
    // for every file on it. Only one of those is recoverable. Each walked root
    // records the configured spelling that settled to it, which identifies an
    // unavailable root however differently it resolves (a mapped network
    // drive, a symlinked folder); a root recorded before that is kept while
    // any configured root cannot be resolved, since nothing proves which
    // configured root produced it. A configured root that resolves is judged
    // by its settled spelling alone.
    let mut keep: Vec<String> = Vec::new();
    let mut unresolved: Vec<&str> = Vec::new();
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
        match settled_root(conn, Path::new(dir)) {
            Ok(settled) => keep.push(settled.to_string_lossy().to_string()),
            Err(_) => unresolved.push(dir),
        }
    }
    let same = |left: &str, right: &str| left == right || left.to_lowercase() == right.to_lowercase();
    let still_configured = |root: &str, configured_root: Option<&str>| {
        keep.iter().any(|spelling| same(spelling, root))
            || match configured_root {
                Some(configured_root) => unresolved.iter().any(|dir| same(dir, configured_root)),
                None => !unresolved.is_empty(),
            }
    };

    // A root still configured as its own source keeps its rows even when it
    // sits inside a root being forgotten here (e.g. the user removes a parent
    // folder but kept a subfolder of it configured separately): the LIKE
    // prefix below would otherwise sweep up that nested source's paths along
    // with the parent's, deleting evidence and rows for a folder the user
    // never asked to stop tracking.
    let kept_roots: Vec<String> = recorded
        .iter()
        .filter(|(root, configured_root)| still_configured(root, configured_root.as_deref()))
        .map(|(root, _)| root.clone())
        .collect();

    let mut forgotten = 0u64;
    for (root, configured_root) in recorded {
        if still_configured(&root, configured_root.as_deref()) {
            continue;
        }
        let nested_kept_roots: Vec<&String> = kept_roots
            .iter()
            .filter(|kept| directory_belongs_to_root(kept, &root) && kept.as_str() != root)
            .collect();
        // `?1` is always the forgotten root's own prefix; `?2`, `?3`, ... are
        // any nested still-configured roots, bound (never interpolated) like
        // every other path value in this function.
        let mut abs_path_params: Vec<String> = vec![like_prefix(&root)];
        let mut exclude_nested_kept = String::new();
        for kept in &nested_kept_roots {
            abs_path_params.push(like_prefix(kept));
            exclude_nested_kept.push_str(&format!(
                " AND abs_path NOT LIKE ?{} ESCAPE '!'",
                abs_path_params.len()
            ));
        }
        // Evidence, companions, paths, orphan contents and scan_dirs all
        // move in one IMMEDIATE batch transaction: a crash or DB error
        // between steps can no longer leak a `contents` row or cache file
        // with no surviving path, and the bulk companion/path writes go
        // through the projection-batch publisher instead of firing the
        // per-row logical-projection trigger for the whole root.
        let mut removed = 0usize;
        let mut orphaned_hashes: Vec<String> = Vec::new();
        crate::index_store::publish_paths_batch(
            conn,
            |tx| {
                tx.execute(
                    "INSERT OR IGNORE INTO batch_touched_hashes \
                     SELECT DISTINCT content_hash FROM paths \
                     WHERE abs_path LIKE ?1 ESCAPE '!' AND content_hash IS NOT NULL",
                    [like_prefix(&root)],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            },
            |tx| {
                // Companions first: their rows hold a foreign key to the primary.
                tx.execute(
                    &format!(
                        "DELETE FROM evidence WHERE path_id IN \
                         (SELECT id FROM paths WHERE abs_path LIKE ?1 ESCAPE '!'{exclude_nested_kept})"
                    ),
                    rusqlite::params_from_iter(abs_path_params.iter()),
                )
                .map_err(|e| e.to_string())?;
                tx.execute(
                    &format!(
                        "UPDATE paths SET companion_of = NULL \
                         WHERE abs_path LIKE ?1 ESCAPE '!' AND companion_of IS NOT NULL{exclude_nested_kept}"
                    ),
                    rusqlite::params_from_iter(abs_path_params.iter()),
                )
                .map_err(|e| e.to_string())?;
                removed = tx
                    .execute(
                        &format!(
                            "DELETE FROM paths WHERE abs_path LIKE ?1 ESCAPE '!'{exclude_nested_kept}"
                        ),
                        rusqlite::params_from_iter(abs_path_params.iter()),
                    )
                    .map_err(|e| e.to_string())?;
                tx.execute("DELETE FROM scan_dirs WHERE root = ?1", [&root])
                    .map_err(|e| e.to_string())?;

                // Orphan collection stays set-based inside SQL end to end: the
                // IN-clauses below are subqueries over `batch_touched_hashes`,
                // never a bound parameter per orphaned hash. A root with more
                // unique items than SQLite's ~32,766 bound-parameter ceiling
                // could not otherwise be forgotten (R6-01). `orphaned_hashes`
                // is read out separately, only so cache files can be removed
                // by hash; it never feeds a query's parameter list.
                const ORPHAN_HASHES: &str = "SELECT content_hash FROM batch_touched_hashes bth \
                     WHERE NOT EXISTS (\
                         SELECT 1 FROM paths lp \
                         WHERE lp.content_hash = bth.content_hash AND lp.missing = 0)";
                let mut stmt = tx.prepare(ORPHAN_HASHES).map_err(|e| e.to_string())?;
                orphaned_hashes = stmt
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| e.to_string())?;
                drop(stmt);
                // Companions first: their rows hold a foreign key to the
                // primary path, which is about to leave `paths` too.
                tx.execute(
                    &format!(
                        "DELETE FROM evidence WHERE path_id IN \
                         (SELECT id FROM paths WHERE content_hash IN ({ORPHAN_HASHES}))"
                    ),
                    [],
                )
                .map_err(|e| e.to_string())?;
                tx.execute(
                    &format!(
                        "UPDATE paths SET companion_of = NULL WHERE companion_of IN \
                         (SELECT id FROM paths WHERE content_hash IN ({ORPHAN_HASHES}))"
                    ),
                    [],
                )
                .map_err(|e| e.to_string())?;
                tx.execute(
                    &format!("DELETE FROM paths WHERE content_hash IN ({ORPHAN_HASHES})"),
                    [],
                )
                .map_err(|e| e.to_string())?;
                tx.execute(
                    &format!(
                        "DELETE FROM similar_group_members WHERE content_hash IN ({ORPHAN_HASHES})"
                    ),
                    [],
                )
                .map_err(|e| e.to_string())?;
                tx.execute(
                    &format!("DELETE FROM contents WHERE hash IN ({ORPHAN_HASHES})"),
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            },
        )?;
        forgotten += removed as u64;
        for hash in &orphaned_hashes {
            crate::preview::remove_entries(cache, hash);
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
    let canonical = crate::volume_io::canonicalize(configured)
        .map_err(|e| format!("{}: {e}", configured.display()))?;
    let canonical_str = canonical.to_string_lossy().to_string();

    let mut stmt = conn
        .prepare("SELECT root FROM scan_dirs")
        .map_err(|e| e.to_string())?;
    let known: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);

    for root in known {
        if root != canonical_str && root.to_lowercase() == canonical_str.to_lowercase() {
            return Ok(PathBuf::from(root));
        }
    }
    Ok(canonical)
}

/// Reconciles configured source folders without completing newly discovered
/// file information. Each successful walk is durable, and the later
/// file-information owner consumes the resulting pending rows and dirty
/// relationship receipts.
pub fn run_source_check(
    conn: &Connection,
    settings: &ScanSettings,
    progress: &dyn Fn(ScanProgress),
) -> Result<ScanSummary, String> {
    run_source_check_scoped(conn, settings, None, progress)
}

/// A recovery keeps the complete configuration authority but walks only the
/// affected configured roots. Narrowing settings would forget unrelated roots.
pub fn run_source_check_scoped(
    conn: &Connection,
    settings: &ScanSettings,
    roots: Option<&[String]>,
    progress: &dyn Fn(ScanProgress),
) -> Result<ScanSummary, String> {
    let _awake = crate::sleep_prevention::begin_work();
    let mut summary = ScanSummary::default();

    // Reconcile configuration BEFORE walking: a root the user removed should
    // stop appearing the moment the next scan runs, not linger until someone
    // notices its files in a section.
    let cache = crate::preview::CachePaths::new(settings.cache_root.clone());
    if roots.is_none() { forget_unconfigured_roots(conn, &settings.source_dirs, &cache)?; }

    let checked_roots: Vec<String> = settings.source_dirs.iter()
        .filter(|root| roots.is_none_or(|scope| scope.contains(root)))
        .cloned().collect();
    let root_total = checked_roots.len() as u64;
    let visibility_roots = crate::visibility_index::source_root_spellings(conn, &settings.source_dirs)?;
    let mut walk_failures = 0u64;
    progress(ScanProgress::phase(ScanPhase::Walk, root_total, None));
    for (root_index, root) in checked_roots.iter().enumerate() {
        // The volume-substitution gate is per root, not per source pass
        // (R3-07, R1-14): a backup drive swapped mid-session at one
        // configured path must not be walked under the original drive's
        // rows, but every other independent root still checks normally.
        if let Err(error) =
            crate::volume::enforce_no_substitution(settings.data_root(), std::slice::from_ref(root))
        {
            check_cancel()?;
            record_issue(conn, Some(root.clone()), WALK_ERROR, &error)?;
            walk_failures += 1;
            summary.failures += 1;
            progress(ScanProgress::walk(
                root_index as u64 + 1,
                root_total,
                root,
                0,
                walk_failures,
            ));
            continue;
        }
        // One settled spelling per root, so a re-typed capitalisation cannot
        // index the same files a second time. Failure belongs to this root,
        // not the whole source pass: record it durably and continue with every
        // independent configured root. Do not walk the unresolved spelling,
        // because an absent symlink or differently-cased alias could create a
        // second scan_dirs identity beside the previously settled root.
        let root = match settled_root(conn, Path::new(root)) {
            Ok(root) => root,
            Err(error) => {
                check_cancel()?;
                record_issue(conn, Some(root.clone()), WALK_ERROR, &error)?;
                walk_failures += 1;
                summary.failures += 1;
                progress(ScanProgress::walk(
                    root_index as u64 + 1,
                    root_total,
                    root,
                    0,
                    walk_failures,
                ));
                continue;
            }
        };
        let configured_root = checked_roots[root_index].as_str();
        let root = root.to_string_lossy().to_string();
        let root = root.as_str();
        let stats = walk_root_with_progress(
            conn,
            Path::new(root),
            configured_root,
            &settings.lists,
            &visibility_roots,
            root_index as u64,
            root_total,
            walk_failures,
            Some(settings.data_root()),
            progress,
        )?;
        if stats.errors == 0 {
            crate::index_store::clear_issues(conn, configured_root, &["watcher-recovery-failed"])?;
        }
        summary.roots += 1;
        summary.seen += stats.seen;
        summary.added += stats.added;
        walk_failures += stats.errors;
        summary.failures += stats.errors;
    }

    // Force one later companion projection even when a walk changed only
    // absence or relationships rather than creating ordinary pending rows.
    begin_scoped_index_repair(conn, &checked_roots)?;
    Ok(summary)
}

pub fn pending_index_work_exists(conn: &Connection) -> Result<bool, String> {
    let probe = |sql: &str| -> Result<bool, String> {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .map(|n| n != 0)
            .map_err(|e| e.to_string())
    };
    if probe(
        "SELECT EXISTS(SELECT 1 FROM paths WHERE missing = 0 AND content_hash IS NULL \
         AND hash_attempt_failed = 0 AND kind IN ('image', 'video', 'audio'))",
    )? {
        return Ok(true);
    }
    if probe(
        "SELECT EXISTS(SELECT 1 FROM paths \
         WHERE missing = 0 AND indexed_at_utc IS NULL AND metadata_attempt_failed = 0)",
    )? {
        return Ok(true);
    }
    if probe(
        "SELECT EXISTS(SELECT 1 FROM paths WHERE missing = 0 \
         AND indexed_at_utc IS NOT NULL AND resolved_source IS NULL)",
    )? {
        return Ok(true);
    }
    if probe("SELECT EXISTS(SELECT 1 FROM paths WHERE missing = 0 AND visibility_checked = 0)")? {
        return Ok(true);
    }
    if probe("SELECT EXISTS(SELECT 1 FROM scan_dirs WHERE relationship_dirty = 1)")? {
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
    let recovery_dirs = pending_index_dirs(conn)?;
    let mut repair_roots = pending_relationship_roots(conn)?;
    repair_roots.extend(begin_scoped_index_repair(conn, &recovery_dirs)?);
    repair_roots.sort();
    repair_roots.dedup();
    run_index_tail_scoped(conn, settings, None, progress, summary)?;
    complete_scoped_index_repair(conn, &repair_roots)
}

/// The same durable tail for a watcher/section repair. Directory scope applies
/// only to pairing: hashing, extraction, and date resolution continue to drain
/// every owed row, and their directories are captured before those receipts
/// disappear so the final relationship projection cannot miss older debt.
pub fn run_index_tail_for_dirs(
    conn: &Connection,
    settings: &ScanSettings,
    dirs: &[String],
    progress: &dyn Fn(ScanProgress),
    summary: &mut ScanSummary,
) -> Result<(), String> {
    let mut pair_dirs = pending_index_dirs(conn)?;
    pair_dirs.extend(dirs.iter().cloned());
    pair_dirs.sort();
    pair_dirs.dedup();
    with_scoped_index_repair(conn, &pair_dirs, || {
        run_index_tail_scoped(conn, settings, Some(&pair_dirs), progress, summary)
    })
}

fn run_index_tail_scoped(
    conn: &Connection,
    settings: &ScanSettings,
    pair_dirs: Option<&[String]>,
    progress: &dyn Fn(ScanProgress),
    summary: &mut ScanSummary,
) -> Result<(), String> {
    let _awake = crate::sleep_prevention::begin_work();
    crate::visibility_index::complete_missing_facts(conn, &settings.source_dirs)?;
    let cache = crate::preview::CachePaths::new(settings.cache_root.clone());
    let hash_stats = hash_pending_with_progress(conn, &cache, progress)?;
    summary.full_hashed = hash_stats.full_hashed;
    summary.copies_disagree = hash_stats.copies_disagree;
    summary.failures += hash_stats.errors;

    summary.failures += extract_pending_with_progress(conn, progress)?.failed;

    let resolve_stats = resolve_from_evidence_with_progress(
        conn,
        &settings.resolution,
        ResolveScope::PendingOnly,
        progress,
    )?;
    summary.resolved = resolve_stats.resolved;
    summary.undated = resolve_stats.undated;

    let pair_stats =
        pair_companions_with_progress(conn, settings.pairing_enabled, pair_dirs, progress)?;
    summary.paired = pair_stats.paired;

    progress(ScanProgress::completed(
        ScanPhase::Indexed,
        1,
        summary.failures,
        None,
    ));

    Ok(())
}

fn pending_index_dirs(conn: &Connection) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT DISTINCT p.dir_path FROM paths p
             WHERE p.missing = 0 AND (
               (p.content_hash IS NULL AND p.kind IN ('image', 'video', 'audio'))
               OR p.indexed_at_utc IS NULL
               OR (p.indexed_at_utc IS NOT NULL AND p.resolved_source IS NULL)
             )",
        )
        .map_err(|error| error.to_string())?;
    let mut dirs = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    dirs.extend(pending_relationship_roots(conn)?);
    dirs.sort();
    dirs.dedup();
    Ok(dirs)
}

fn pending_relationship_roots(conn: &Connection) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare("SELECT root FROM scan_dirs WHERE relationship_dirty = 1 ORDER BY root")
        .map_err(|error| error.to_string())?;
    let roots = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(roots)
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

/// The one per-file upsert: classify + insert/update/skip-unchanged over the
/// file's metadata, read by the caller through `volume_io` (the walk reads it
/// on its own thread). Shared by the full walk and the watcher's
/// single-directory re-stat, so the two can never drift on the checkpoint
/// semantics (size+mtime unchanged = skip, changed = reset content facts).
pub fn upsert_file(
    conn: &Connection,
    path: &Path,
    meta: &std::fs::Metadata,
    lists: &ScanLists,
    inherited_visibility_flags: i64,
) -> Result<Upsert, String> {
    let abs = path.to_string_lossy().to_string();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let own_visibility_flags = crate::visibility::entry_flags(path, meta);
    let visibility_flags = own_visibility_flags | inherited_visibility_flags;
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
    let kind = extensions::classify(
        &ext,
        &lists.images,
        &lists.videos,
        &lists.audio,
        &lists.companions,
    )
    .as_str();
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
            let changed = conn.execute(
                "UPDATE paths SET missing = 0, own_visibility_flags = ?2, visibility_flags = ?3, visibility_checked = 1
                 WHERE abs_path = ?1 AND (missing = 1 OR own_visibility_flags != ?2 OR visibility_flags != ?3 OR visibility_checked != 1)",
                params![abs, own_visibility_flags, visibility_flags],
            )
            .map_err(|e| e.to_string())?;
            Ok(if changed == 0 { Upsert::Unchanged } else { Upsert::Updated })
        }
        Some(_) => {
            // A provisional key is `p<path_id>` — derived from the path, not
            // the bytes — so an in-place replacement regenerates the SAME key.
            // Left alone, the old contents row hands the new file the previous
            // file's facts: byte_size, phash, sharpness, strip_frames and,
            // fatally, derived_at_utc and derive_outcome, which make both
            // derive passes skip it for the life of the index. That is why
            // re-saving a trimmed clip kept showing the old poster and strip,
            // and why a rescan did not fix it. Captured here, dropped after
            // the row detaches below — paths.content_hash is a foreign key
            // into contents, so deleting first is a constraint violation.
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
                 hash_attempt_failed = 0, metadata_attempt_failed = 0, \
                 indexed_at_utc = NULL, resolved_utc_ms = NULL, resolved_source = NULL, \
                 date_only = 0, missing = 0, own_visibility_flags = ?8, visibility_flags = ?9, visibility_checked = 1 WHERE abs_path = ?1",
                params![abs, size, mtime_ms, birthtime_ms, ext, kind, stem, own_visibility_flags, visibility_flags],
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
                 mtime_ms, birthtime_ms, missing, own_visibility_flags, visibility_flags, visibility_checked)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11, 1)",
                params![abs, dir_path, file_name, stem, ext, kind, size, mtime_ms, birthtime_ms, own_visibility_flags, visibility_flags],
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
    walk_root_with_progress(conn, root, &root.to_string_lossy(), lists, &[crate::winpath::for_fs(root).to_string_lossy().into_owned()], 0, 1, 0, None, &|_| {})
}

/// Excludes the app's own storage the way `trash::is_trash_path` excludes
/// deleted-file storage: a source containing the data root must never index
/// or churn the app's own index, logs, caches and models (R6-02).
fn is_excluded_from_discovery(path: &Path, data_root: Option<&Path>) -> bool {
    crate::trash::is_trash_path(path)
        || data_root.is_some_and(|root| crate::paths::is_within_data_root(path, root))
        || is_apple_double_sidecar(path)
        || crate::file_identity::is_private_tmp_name(path)
}

/// Whether `path` is a macOS AppleDouble sidecar (`._name`) sitting beside its
/// real file `name` in the same directory. macOS writes these to carry
/// extended attributes and resource forks on a volume that cannot store them
/// natively (FAT, exFAT, many network shares); they are operating-system
/// metadata, never library content, so they are excluded from discovery
/// wherever trash and the data root are (plan decision; library-maintenance.md).
/// Trashing the real file leaves this indexed row to fail as missing, which
/// is the concrete defect the exclusion closes. The sibling check keeps this
/// from ever excluding a real file or directory that merely happens to start
/// with `._` and has nothing named after the rest of it.
pub(crate) fn is_apple_double_sidecar(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    match name.strip_prefix("._") {
        // A sibling that cannot be checked is not proven: index the entry.
        Some(real_name) if !real_name.is_empty() => {
            crate::volume_io::exists(&path.with_file_name(real_name)).unwrap_or(false)
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_root_with_progress(
    conn: &Connection,
    root: &Path,
    configured_root: &str,
    lists: &ScanLists,
    visibility_roots: &[String],
    completed_roots: u64,
    total_roots: u64,
    failures_before: u64,
    data_root: Option<&Path>,
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
        "INSERT INTO scan_dirs (root, dirty, configured_root) VALUES (?1, 1, ?2) \
         ON CONFLICT(root) DO UPDATE SET dirty = 1, configured_root = excluded.configured_root",
        params![root_str, configured_root],
    )
    .map_err(|e| e.to_string())?;

    // A connection-local table makes the complete-walk absence publication
    // bounded by SQLite rather than by a million-path Rust set plus one durable
    // transaction per vanished file. An interrupted walk never reaches that
    // publication boundary, so its partial table proves no absences.
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS walk_present_paths (\
             abs_path TEXT PRIMARY KEY\
         ) WITHOUT ROWID;\
         CREATE TEMP TABLE IF NOT EXISTS walk_vanished_paths (\
             abs_path TEXT PRIMARY KEY,\
             content_hash TEXT\
         ) WITHOUT ROWID;\
         DELETE FROM walk_present_paths;\
         DELETE FROM walk_vanished_paths;",
    )
    .map_err(|error| error.to_string())?;
    let mut record_present = conn
        .prepare_cached("INSERT OR IGNORE INTO walk_present_paths (abs_path) VALUES (?1)")
        .map_err(|error| error.to_string())?;
    let mut walk_incomplete = false;
    let mut current_stat_failures = std::collections::HashSet::<String>::new();
    // One probe up front so a clean index never pays a per-file DELETE.
    let mut issues_present = crate::index_store::any_issues(conn)?;
    let mut visibility_directories = crate::visibility_index::DirectoryFacts::default();

    // The walk root carries the long-path form so every entry beneath it
    // inherits it; without this a deep tree is simply invisible on Windows.
    // Entries carry the root's resolved spelling (verbatim on Windows, links
    // resolved), so the data root is excluded in that same spelling.
    let resolved_data_root = data_root.map(|root| {
        std::fs::canonicalize(crate::winpath::for_fs(root).as_ref()) // data root
            .unwrap_or_else(|_| root.to_path_buf())
    });
    // The walk runs on a volume_io thread and hands entries over with their
    // metadata; each wait for the next entry is bounded. The filter runs on
    // that thread too, so the calls it makes run there directly.
    let mut walk = match crate::volume_io::walk(fs_root.as_ref(), move |path, file_type| {
            if file_type.is_file()
                && crate::file_identity::is_private_tmp_name(path)
                && crate::file_identity::is_abandoned_leftover(path)
            {
                // This application home's own launch-time garbage, left by a
                // previous process that quitting gave up on; see
                // `file_identity::is_abandoned_leftover`. A different
                // application home's in-progress file in a folder two homes
                // share, or a still-running process's own file, is proven not
                // ours and is left untouched — a live private file must never
                // be deleted merely because a walk happened to visit it.
                crate::fs_recovery::remove_file(path, "private staging leftover cleanup");
            }
            !is_excluded_from_discovery(path, resolved_data_root.as_deref())
        }) {
        Ok(walk) => Some(walk),
        Err(error) => {
            // The volume is already known not to answer: nothing was read,
            // so nothing is proven absent.
            walk_incomplete = true;
            stats.errors += 1;
            record_issue(conn, Some(root_str.clone()), WALK_ERROR, &error.to_string())?;
            None
        }
    };
    while let Some(next) = walk.as_mut().and_then(|walk| walk.next(Some(&cancelled))) {
        check_cancel()?;
        let entry = match next {
            Ok(Ok(entry)) => entry,
            Ok(Err(err)) => {
                walk_incomplete = true;
                stats.errors += 1;
                record_issue(
                    conn,
                    err.path.map(|p| p.to_string_lossy().to_string()),
                    WALK_ERROR,
                    &err.message,
                )?;
                continue;
            }
            // Given up on: the volume stopped answering. The walk ends here,
            // incomplete, so no absence is published and the root stays
            // owed; the claim is released as this function returns.
            Err(error) => {
                check_cancel()?;
                walk_incomplete = true;
                stats.errors += 1;
                record_issue(conn, Some(root_str.clone()), WALK_ERROR, &error.to_string())?;
                break;
            }
        };
        // A pending foreground action or urgent preparation takes the index
        // here and the walk continues from this entry afterwards. Rows and
        // attributes it cached may have changed meanwhile.
        if crate::scan_runtime::yield_at_safe_point()? {
            visibility_directories = crate::visibility_index::DirectoryFacts::default();
            issues_present = crate::index_store::any_issues(conn)?;
        }
        if issues_present {
            let entry_path = entry.path.to_string_lossy().to_string();
            crate::index_store::clear_issues(conn, &entry_path, &[WALK_ERROR])?;
        }
        if !entry.file_type.is_file() {
            continue;
        }
        let path = entry.path.as_path();
        let abs = path.to_string_lossy().to_string();
        stats.seen += 1;
        record_present
            .execute([&abs])
            .map_err(|error| error.to_string())?;

        let upsert = (|| {
            let visibility_root = crate::visibility_index::root_for(visibility_roots, path).ok_or("File is outside configured sources")?;
            let inherited = visibility_directories.refresh(conn, &visibility_root, path.parent().ok_or("File has no parent")?)?;
            let meta = match &entry.metadata {
                Some(Ok(meta)) => meta,
                Some(Err(error)) => return Err(error.to_string()),
                None => return Err("File metadata was not read".to_string()),
            };
            upsert_file(conn, path, meta, lists, inherited)
        })();
        match upsert {
            Ok(outcome) => {
                match outcome {
                    Upsert::Added => stats.added += 1,
                    Upsert::Updated => stats.updated += 1,
                    Upsert::Unchanged => stats.unchanged += 1,
                }
                // The success counterpart: a re-walked file that now stats
                // clean drops its scan-condition rows (current-state issues).
                if issues_present {
                    crate::index_store::clear_issues(conn, &abs, &[STAT_ERROR, WALK_ERROR])?;
                }
            }
            Err(_) if vanished(path) => {
                // Listed, then removed before its stat: a walk parked for a
                // foreground action resumes with entries that action may
                // have deleted or moved. The file is absent, not failing.
                stats.seen -= 1;
                conn.execute("DELETE FROM walk_present_paths WHERE abs_path = ?1", [&abs])
                    .map_err(|error| error.to_string())?;
            }
            Err(err) => {
                stats.seen -= 1;
                // WalkDir proved this pathname exists even though its richer
                // stat/upsert failed. Keep it in `present`: absence is false,
                // while the exact stat condition remains a recheckable Issue.
                current_stat_failures.insert(abs.clone());
                stats.errors += 1;
                record_issue(conn, Some(abs), STAT_ERROR, &err)?;
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
    drop(record_present);

    // Only a complete directory enumeration can prove absence. A path whose
    // richer stat failed was still seen and remains live with an exact Issue;
    // only a WalkDir traversal error can hide an unknown subtree, so only that
    // leaves the root dirty and suppresses the missing-row diff.
    if !walk_incomplete {
        // Anything under this root the walk did not see is missing. The rows
        // stay (their trash/delete history may matter) but leave every view
        // and count. LIKE wildcards in the root itself (`_` is common in real
        // paths) are escaped with `!`, which appears in no sane path.
        let placeholders_root = like_prefix(&root_str);
        check_cancel()?;
        crate::index_store::publish_paths_batch(
            conn,
            |tx| {
                tx.execute(
                    "INSERT INTO walk_vanished_paths (abs_path, content_hash) \
                     SELECT paths.abs_path, paths.content_hash FROM paths \
                     WHERE paths.abs_path LIKE ?1 ESCAPE '!' AND paths.missing = 0 \
                     AND NOT EXISTS (\
                         SELECT 1 FROM walk_present_paths \
                         WHERE walk_present_paths.abs_path = paths.abs_path\
                     )",
                    [&placeholders_root],
                )
                .map_err(|error| error.to_string())?;
                tx.execute(
                    "INSERT OR IGNORE INTO batch_touched_hashes \
                     SELECT content_hash FROM walk_vanished_paths WHERE content_hash IS NOT NULL",
                    [],
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
            },
            |tx| {
                close_path_scan_issues(
                    tx,
                    "path IN (SELECT abs_path FROM walk_vanished_paths)",
                    &[],
                )?;
                stats.marked_missing += tx
                    .execute(
                        "UPDATE paths SET missing = 1 \
                         WHERE abs_path IN (SELECT abs_path FROM walk_vanished_paths)",
                        [],
                    )
                    .map_err(|error| error.to_string())? as u64;
                Ok(())
            },
        )?;

        // A complete walk also proves that old entry failures beneath this
        // root no longer exist, including when the failed entry itself was
        // removed and therefore yielded no success event above.
        crate::index_store::close_issues(
            conn,
            "resolved",
            "kind = ?1 AND (path = ?2 OR path LIKE ?3 ESCAPE '!')",
            params![WALK_ERROR, root_str, placeholders_root],
        )?;

        // Successful files cleared their own stat row above. The remaining
        // stale rows can only name entries a complete traversal proved gone;
        // preserve the exact paths that failed again in this pass.
        let mut statement = conn
            .prepare(
                "SELECT path FROM active_issues WHERE kind = ?1 \
                 AND (path = ?2 OR path LIKE ?3 ESCAPE '!')",
            )
            .map_err(|error| error.to_string())?;
        let stale_stat_paths = statement
            .query_map(params![STAT_ERROR, root_str, placeholders_root], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|path| !current_stat_failures.contains(path))
            .collect::<Vec<_>>();
        drop(statement);
        for path in stale_stat_paths {
            crate::index_store::clear_issues(conn, &path, &[STAT_ERROR])?;
        }

        conn.execute(
            "INSERT INTO scan_dirs (root, last_completed_at_utc, dirty) VALUES (?1, ?2, 0) \
             ON CONFLICT(root) DO UPDATE SET last_completed_at_utc = ?2, dirty = 0",
            params![root_str, scanned_at],
        )
        .map_err(|e| e.to_string())?;
    }

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

/// Complete content bytes are a logical item's identity, and empty content
/// carries no evidence: every zero-byte file stays an individual physical
/// file (a provisional media key or an unhashed Other file), so acting on one
/// empty file never reaches another.
pub fn carries_content_identity(byte_size: i64) -> bool {
    byte_size > 0
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
    // One IMMEDIATE transaction is the single owner of this decision: it
    // re-reads `already_known` after taking the write lock, so two
    // concurrent promoters of the same provisional key never both see
    // `false` and race the `contents` primary key. Cache files move only
    // after commit, keyed by the outcome the committed transaction decided.
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let byte_size: Option<i64> = tx
        .query_row(
            "SELECT byte_size FROM contents WHERE hash = ?1",
            [provisional],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if byte_size.is_some_and(|size| !carries_content_identity(size)) {
        // An empty file keeps its individual provisional identity; a full
        // hash of nothing would merge it with every other empty file.
        return Ok(());
    }
    let already_known: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM contents WHERE hash = ?1)",
            [real_hash],
            |r| r.get::<_, i64>(0).map(|n| n != 0),
        )
        .map_err(|e| e.to_string())?;

    // Expensive results follow the identity; the real key's own result wins.
    tx.execute_batch(&format!(
        "UPDATE OR IGNORE transcripts SET content_hash = {real} WHERE content_hash = {provisional};
         UPDATE faces SET content_hash = {real} WHERE content_hash = {provisional}
           AND NOT EXISTS (SELECT 1 FROM face_checks WHERE content_hash = {real});
         UPDATE OR IGNORE face_checks SET content_hash = {real} WHERE content_hash = {provisional};",
        real = crate::records::sql_text(Some(real_hash)),
        provisional = crate::records::sql_text(Some(provisional)),
    ))
    .map_err(|e| e.to_string())?;
    let strip_frames_for_rename = if already_known {
        tx.execute(
            "UPDATE paths SET content_hash = ?2 WHERE content_hash = ?1",
            params![provisional, real_hash],
        )
        .map_err(|e| e.to_string())?;
        // The logical-content triggers dirty the affected cohort; dropping
        // provisional membership avoids duplicating the real row meanwhile.
        tx.execute(
            "DELETE FROM similar_group_members WHERE content_hash = ?1",
            [provisional],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM contents WHERE hash = ?1", [provisional])
            .map_err(|e| e.to_string())?;
        None
    } else {
        // The FK from paths forbids renaming the parent in place: copy the
        // row under the real key, repoint the children, drop the old row.
        tx.execute(
            "INSERT INTO contents (hash, byte_size, kind, phash, camera_make, camera_model, \
             width, height, duration_ms, sharpness, strip_frames, derived_at_utc, derive_outcome) \
             SELECT ?2, byte_size, kind, phash, camera_make, camera_model, \
             width, height, duration_ms, sharpness, strip_frames, derived_at_utc, derive_outcome \
             FROM contents WHERE hash = ?1",
            params![provisional, real_hash],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE paths SET content_hash = ?2 WHERE content_hash = ?1",
            params![provisional, real_hash],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE similar_group_members SET content_hash = ?2 WHERE content_hash = ?1",
            params![provisional, real_hash],
        )
        .map_err(|e| e.to_string())?;
        let strip_frames: Option<i64> = tx
            .query_row(
                "SELECT strip_frames FROM contents WHERE hash = ?1",
                [real_hash],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM contents WHERE hash = ?1", [provisional])
            .map_err(|e| e.to_string())?;
        Some(strip_frames.unwrap_or(0))
    };
    // Held from just before commit through the cache rename/removal below, so
    // the startup sweep (which takes the same lock for its whole pass) can
    // never run its "is this key still in `contents`?" check in the gap
    // between this commit and the cache files actually moving — otherwise it
    // can see the just-vacated provisional key as orphaned and delete the
    // file this function is about to rename out from under it.
    let _identity_lock = crate::preview::lock_cache_identity();
    crate::records::commit(tx).map_err(|e| e.to_string())?;

    if let Some(strip_frames) = strip_frames_for_rename {
        crate::preview::rename_entries(cache, provisional, real_hash, strip_frames);
    } else {
        crate::preview::remove_entries(cache, provisional);
    }
    drop(_identity_lock);
    let _ = crate::activity::record(crate::activity::ActivityDraft {
        kind: crate::activity::ActivityKind::Changed,
        owner: crate::activity::ActivityOwner::Identity,
        subject: None,
        operation_id: None,
        cause_id: None,
        generation: None,
        previous: None,
        current: Some(crate::activity::ActivityState::Succeeded),
        reason: Some(crate::activity::ActivityReason::Completion),
        lane: None,
        item_count: Some(1),
        queued: None,
        done: None,
        total: None,
        target_hash: None,
    });
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
             WHERE missing = 0 AND hash_attempt_failed = 0 AND (content_hash IS NULL OR content_hash GLOB 'p*')",
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
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
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
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()
        .map_err(|e| e.to_string())?;
    drop(known_stmt);

    let needs_content_identity = |kind: &str| matches!(kind, "image" | "video" | "audio");

    // Assigns the resting identity of a row that nothing collides with.
    let settle_unique = |row: &Row, stats: &mut HashStats| -> Result<(), String> {
        if row.provisional.is_some() {
            return Ok(()); // already identified, still unique
        }
        if needs_content_identity(&row.kind) {
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
    let issues_present = crate::index_store::any_issues(conn)?;
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
                crate::information_attempts::failed(conn, row.id, &row.abs, crate::information_attempts::Stage::Identity, &err.to_string())?;
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
        if !carries_content_identity(size) {
            // Empty files never collapse: each stays its own physical item.
            for row in &group {
                check_cancel()?;
                report_path(row, done, stats.errors, None, None);
                settle_unique(row, &mut stats)?;
                done += 1;
                report_path(row, done, stats.errors, None, None);
            }
            continue;
        }
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
                        crate::information_attempts::failed(conn, row.id, &row.abs, crate::information_attempts::Stage::Identity, &err.to_string())?;
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
                            "{group_len} same-size same-prehash files split into {} distinct contents (size {size})",
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
    pub failed: u64,
}

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
         WHERE missing = 0 AND indexed_at_utc IS NULL AND metadata_attempt_failed = 0",
    )?;
    let total = rows.len() as u64;
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
            stats.failed,
            ScanPhase::Resolve,
        ));
        let path = Path::new(&abs);
        let meta = match kind.as_str() {
            "image" => metadata::read_image_metadata(path).map(Some),
            "video" => metadata::read_video_metadata(path).map(Some),
            // Companion RAW files are TIFF containers with readable EXIF.
            "companion" => metadata::read_image_metadata(path).map(Some),
            _ => Ok(None),
        };
        let meta = match meta {
            Ok(meta) => meta,
            Err(error) => {
                crate::information_attempts::failed(
                    conn,
                    id,
                    &abs,
                    crate::information_attempts::Stage::Metadata,
                    &error.to_string(),
                )?;
                stats.failed += 1;
                done += 1;
                continue;
            }
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
        crate::index_store::clear_issues(
            conn,
            &abs,
            &[crate::information_attempts::Stage::Metadata.issue_kind()],
        )?;
        stats.extracted += 1;
        done += 1;
        progress(ScanProgress::at_path(
            ScanPhase::Extract,
            done,
            total,
            &abs,
            stats.failed,
            ScanPhase::Resolve,
        ));
    }

    progress(ScanProgress::completed(
        ScanPhase::Extract,
        total,
        stats.failed,
        Some(ScanPhase::Resolve),
    ));

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

pub const RESOLVE_PAGE_SIZE: usize = 256;

/// The pure resolution pass: stored evidence + stat columns → resolved
/// timestamp columns. Never opens a file.
pub fn resolve_from_evidence(
    conn: &Connection,
    config: &ResolutionConfig,
    scope: ResolveScope,
) -> Result<ResolveStats, String> {
    resolve_from_evidence_with_progress(conn, config, scope, &|_| {})
}

/// Settings changed the resolution policy. Invalidate the old projection
/// first, then rebuild through the ordinary pending path so cancellation or
/// a crash leaves explicit resumable debt instead of a library silently
/// split between old and new rules.
pub fn re_resolve_all_with_progress(
    conn: &Connection,
    config: &ResolutionConfig,
    pairing_enabled: bool,
    progress: &dyn Fn(ScanProgress),
) -> Result<ResolveStats, String> {
    check_cancel()?;
    // A settings-wide re-resolve touches every indexed row. One statement
    // (or one batch covering the whole library) still fires the projection
    // rebuild for every touched hash inside a single transaction, holding
    // the write lock for as long as the library is large. Paging the
    // invalidation the same way the per-file resolve phase pages its
    // writes below keeps every transaction's hold on the write lock
    // bounded by `RESOLVE_PAGE_SIZE`, not by library size, so a concurrent
    // writer's busy_timeout is never outlasted by one statement.
    let mut invalidate_stmt = conn
        .prepare(
            "SELECT id FROM paths \
             WHERE indexed_at_utc IS NOT NULL AND resolved_source IS NOT NULL AND id > ?1 \
             ORDER BY id LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let mut after_id = 0i64;
    loop {
        check_cancel()?;
        let ids: Vec<i64> = invalidate_stmt
            .query_map(params![after_id, RESOLVE_PAGE_SIZE as i64], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        if ids.is_empty() {
            break;
        }
        after_id = *ids.last().unwrap();

        let placeholders = std::iter::repeat("?")
            .take(ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let hash_sql = format!(
            "INSERT OR IGNORE INTO batch_touched_hashes \
             SELECT DISTINCT content_hash FROM paths \
             WHERE content_hash IS NOT NULL AND id IN ({placeholders})"
        );
        let update_sql = format!(
            "UPDATE paths SET resolved_utc_ms = NULL, resolved_source = NULL, date_only = 0 \
             WHERE id IN ({placeholders})"
        );
        let id_params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        crate::index_store::publish_paths_batch(
            conn,
            |tx| {
                tx.execute(&hash_sql, id_params.as_slice())
                    .map_err(|e| e.to_string())?;
                Ok(())
            },
            |tx| {
                tx.execute(&update_sql, id_params.as_slice())
                    .map_err(|e| e.to_string())?;
                Ok(())
            },
        )?;
        // See the matching pause in `resolve_from_evidence_with_progress`:
        // this loop has no other work between one page's commit and the
        // next page's `BEGIN IMMEDIATE`, so without a deliberate pause a
        // concurrent writer can still starve for the whole invalidation
        // even though each page's transaction is brief.
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    drop(invalidate_stmt);
    let stats = resolve_from_evidence_with_progress(
        conn,
        config,
        ResolveScope::PendingOnly,
        progress,
    )?;
    pair_companions_with_progress(conn, pairing_enabled, None, progress)?;
    progress(ScanProgress::completed(
        ScanPhase::Indexed,
        1,
        0,
        None,
    ));
    Ok(stats)
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

    enum RowResolution {
        Resolved {
            unix_ms: i64,
            source: &'static str,
            date_only: bool,
        },
        Undated,
    }

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

        // Compute this page's resolutions outside any transaction, then
        // publish them all in one IMMEDIATE transaction (D-H2): autocommit
        // per row fires the per-row logical-projection trigger for every
        // row and starves concurrent writers on a large library.
        let mut page_ids: Vec<i64> = Vec::with_capacity(rows.len());
        let mut page_results: Vec<(i64, RowResolution)> = Vec::with_capacity(rows.len());
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

            let resolution = match resolution::resolve(meta_ts, file_ts, mtime_ms, birthtime_ms, config) {
                Some(resolved) => {
                    stats.resolved += 1;
                    RowResolution::Resolved {
                        unix_ms: resolved.unix_ms,
                        source: resolved.source.as_str(),
                        date_only: resolved.date_only,
                    }
                }
                None => {
                    stats.undated += 1;
                    RowResolution::Undated
                }
            };
            page_ids.push(id);
            page_results.push((id, resolution));
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

        let placeholders = std::iter::repeat("?")
            .take(page_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let hash_sql = format!(
            "INSERT OR IGNORE INTO batch_touched_hashes \
             SELECT DISTINCT content_hash FROM paths \
             WHERE content_hash IS NOT NULL AND id IN ({placeholders})"
        );
        let id_params: Vec<&dyn rusqlite::ToSql> =
            page_ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

        crate::index_store::publish_paths_batch(
            conn,
            |tx| {
                tx.execute(&hash_sql, id_params.as_slice())
                    .map_err(|e| e.to_string())?;
                Ok(())
            },
            |tx| {
                let mut resolved_stmt = tx
                    .prepare(
                        "UPDATE paths SET resolved_utc_ms = ?2, resolved_source = ?3, \
                         date_only = ?4 WHERE id = ?1",
                    )
                    .map_err(|e| e.to_string())?;
                let mut undated_stmt = tx
                    .prepare(
                        "UPDATE paths SET resolved_utc_ms = NULL, resolved_source = 'undated', \
                         date_only = 0 WHERE id = ?1",
                    )
                    .map_err(|e| e.to_string())?;
                for (id, resolution) in &page_results {
                    match resolution {
                        RowResolution::Resolved {
                            unix_ms,
                            source,
                            date_only,
                        } => {
                            resolved_stmt
                                .execute(params![id, unix_ms, source, *date_only as i64])
                                .map_err(|e| e.to_string())?;
                        }
                        RowResolution::Undated => {
                            undated_stmt
                                .execute(params![id])
                                .map_err(|e| e.to_string())?;
                        }
                    }
                }
                Ok(())
            },
        )?;
        // Compute alone leaves only a sub-millisecond gap between one
        // page's commit and the next page's `BEGIN IMMEDIATE` (measured:
        // tens to low hundreds of microseconds for a page of evidence
        // reads) — far short of what an OS thread needs to wake from a
        // busy-wait and actually win the reacquired lock, so a concurrent
        // writer can still starve for the whole pass even though every
        // individual transaction is brief. A short deliberate pause here
        // is the difference between a brief transaction and a genuinely
        // idle interval a competing writer can use.
        std::thread::sleep(std::time::Duration::from_millis(2));
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

fn raw_pair_candidates_sql(scoped: bool) -> String {
    let from = if scoped {
        "FROM onecopy_pair_scope scope
         CROSS JOIN paths companion INDEXED BY idx_paths_pairing"
    } else {
        "FROM paths companion"
    };
    let scope = scoped
        .then_some(" AND companion.dir_path = scope.dir_path")
        .unwrap_or("");
    format!(
        "INSERT INTO onecopy_pair_results (path_id, primary_id)
         SELECT companion.id, (
           SELECT candidate.id FROM paths candidate INDEXED BY idx_paths_pairing
           WHERE candidate.dir_path = companion.dir_path
             AND candidate.stem = companion.stem
             AND candidate.kind IN ('image', 'video')
             AND candidate.missing = 0
             AND (candidate.kind != 'video' OR NOT EXISTS (
                   SELECT 1 FROM evidence video_id INDEXED BY idx_evidence_path
                   WHERE video_id.path_id = candidate.id
                     AND video_id.source = 'live-photo-identifier'
                     AND video_id.raw IS NOT NULL
                     AND EXISTS (
                       SELECT 1 FROM paths live_image
                       JOIN evidence image_id INDEXED BY idx_evidence_path ON image_id.path_id = live_image.id
                       WHERE live_image.dir_path = candidate.dir_path
                         AND live_image.kind = 'image' AND live_image.missing = 0
                         AND image_id.source = 'live-photo-identifier'
                         AND image_id.raw = video_id.raw)))
           ORDER BY candidate.abs_path COLLATE onecopy_nocase, candidate.abs_path LIMIT 1)
         {from}
         WHERE companion.kind = 'companion' AND companion.missing = 0{scope}"
    )
}

fn live_photo_pair_candidates_sql(scoped: bool) -> String {
    let from = if scoped {
        "FROM onecopy_pair_scope scope
         CROSS JOIN paths video INDEXED BY idx_paths_pairing"
    } else {
        "FROM paths video"
    };
    let scope = scoped
        .then_some(" AND video.dir_path = scope.dir_path")
        .unwrap_or("");
    format!(
        "INSERT INTO onecopy_pair_results (path_id, primary_id)
         SELECT video.id, (
           SELECT image.id
           FROM paths image INDEXED BY idx_paths_dir
           WHERE image.dir_path = video.dir_path
             AND image.kind = 'image' AND image.missing = 0
             AND EXISTS (
               SELECT 1 FROM evidence image_id INDEXED BY idx_evidence_path
               WHERE image_id.path_id = image.id
                 AND image_id.source = 'live-photo-identifier'
                 AND image_id.raw IS NOT NULL
                 AND EXISTS (
                   SELECT 1 FROM evidence video_id INDEXED BY idx_evidence_path
                   WHERE video_id.path_id = video.id
                     AND video_id.source = 'live-photo-identifier'
                     AND video_id.raw = image_id.raw))
           ORDER BY image.abs_path COLLATE onecopy_nocase, image.abs_path LIMIT 1)
         {from}
         WHERE video.kind = 'video' AND video.missing = 0{scope}"
    )
}

/// Rebuilds every enabled companion relationship. RAW/sidecar companions use
/// same-directory + lowercased stem, and never pick a video that is itself a
/// Live Photo companion movie as their main copy, so a sidecar never becomes
/// a companion of a companion. Live Photo MOVs use same-directory + exact
/// Apple content identifier and may have unrelated stems. An ambiguous match
/// is broken deterministically by path order (case-insensitive, then exact),
/// never by insertion id. Disabled pairing leaves every row independent.
pub fn pair_companions(conn: &Connection, enabled: bool) -> Result<PairStats, String> {
    pair_companions_with_progress(conn, enabled, None, &|_| {})
}

pub fn pair_companions_in_dirs(
    conn: &Connection,
    enabled: bool,
    dirs: &[String],
) -> Result<PairStats, String> {
    pair_companions_with_progress(conn, enabled, Some(dirs), &|_| {})
}

fn pair_companions_with_progress(
    conn: &Connection,
    enabled: bool,
    dirs: Option<&[String]>,
    progress: &dyn Fn(ScanProgress),
) -> Result<PairStats, String> {
    let mut phase = ScanProgress::phase(ScanPhase::Pair, 1, Some(ScanPhase::Indexed));
    progress(phase.clone());
    check_cancel()?;
    // Decide every relationship from one snapshot, then publish the rows
    // whose `companion_of` changes in pages through the projection-batch
    // publisher, so re-pairing a whole library never holds the write lock
    // for its size. An interrupted publication leaves the caller's
    // relationship receipt dirty, and the next repair decides again.
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .map_err(|error| error.to_string())?;
    let paired = (|| -> Result<u64, String> {
        transaction
            .execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS onecopy_pair_scope (
               dir_path TEXT PRIMARY KEY
             ) WITHOUT ROWID;
             CREATE TEMP TABLE IF NOT EXISTS onecopy_pair_results (
               path_id INTEGER PRIMARY KEY,
               primary_id INTEGER
             ) WITHOUT ROWID;
             CREATE TEMP TABLE IF NOT EXISTS onecopy_pair_changes (
               path_id INTEGER PRIMARY KEY,
               companion_of INTEGER
             ) WITHOUT ROWID;
             CREATE TEMP TABLE IF NOT EXISTS onecopy_pair_page (
               path_id INTEGER PRIMARY KEY
             ) WITHOUT ROWID;
             DELETE FROM onecopy_pair_scope;
             DELETE FROM onecopy_pair_results;
             DELETE FROM onecopy_pair_changes;",
            )
            .map_err(|error| error.to_string())?;

        if let Some(dirs) = dirs {
            let mut insert_scope = transaction
                .prepare("INSERT OR IGNORE INTO onecopy_pair_scope (dir_path) VALUES (?1)")
                .map_err(|error| error.to_string())?;
            for dir in dirs {
                insert_scope
                    .execute([dir])
                    .map_err(|error| error.to_string())?;
            }
        }

        let update_scope = dirs
            .is_some()
            .then_some(" AND paths.dir_path IN (SELECT dir_path FROM onecopy_pair_scope)")
            .unwrap_or("");

        if enabled {
            // Target row first, then a same-directory indexed lookup. The
            // scalar subquery prevents duplicate backup trees with the same
            // Apple identifier from forming a fleet-wide evidence cross-product.
            transaction
                .execute(&raw_pair_candidates_sql(dirs.is_some()), [])
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "DELETE FROM onecopy_pair_results WHERE primary_id IS NULL",
                    [],
                )
                .map_err(|error| error.to_string())?;

            transaction
                .execute(&live_photo_pair_candidates_sql(dirs.is_some()), [])
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "DELETE FROM onecopy_pair_results WHERE primary_id IS NULL",
                    [],
                )
                .map_err(|error| error.to_string())?;
        }

        // A scoped companion no longer desired is released; every desired
        // relationship that differs is set.
        transaction
            .execute(
                &format!(
                    "INSERT INTO onecopy_pair_changes (path_id, companion_of)
                 SELECT paths.id, desired.primary_id FROM paths
                 LEFT JOIN onecopy_pair_results desired ON desired.path_id = paths.id
                 WHERE ((paths.companion_of IS NOT NULL{update_scope})
                        OR desired.path_id IS NOT NULL)
                   AND paths.companion_of IS NOT desired.primary_id"
                ),
                [],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .query_row("SELECT COUNT(*) FROM onecopy_pair_results", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count as u64)
            .map_err(|error| error.to_string())
    })()?;
    crate::records::commit(transaction).map_err(|error| error.to_string())?;

    let mut after_id = 0i64;
    loop {
        check_cancel()?;
        let mut last_id = None;
        crate::index_store::publish_paths_batch(
            conn,
            |tx| {
                tx.execute("DELETE FROM onecopy_pair_page", [])
                    .map_err(|error| error.to_string())?;
                tx.execute(
                    "INSERT INTO onecopy_pair_page SELECT path_id FROM onecopy_pair_changes
                     WHERE path_id > ?1 ORDER BY path_id LIMIT ?2",
                    params![after_id, RESOLVE_PAGE_SIZE as i64],
                )
                .map_err(|error| error.to_string())?;
                last_id = tx
                    .query_row("SELECT MAX(path_id) FROM onecopy_pair_page", [], |row| row.get(0))
                    .map_err(|error| error.to_string())?;
                tx.execute(
                    "INSERT OR IGNORE INTO batch_touched_hashes SELECT content_hash FROM paths
                     WHERE content_hash IS NOT NULL AND id IN (SELECT path_id FROM onecopy_pair_page)",
                    [],
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
            },
            |tx| {
                tx.execute(
                    "UPDATE paths SET companion_of = (
                       SELECT change.companion_of FROM onecopy_pair_changes change
                       WHERE change.path_id = paths.id)
                     WHERE id IN (SELECT path_id FROM onecopy_pair_page)",
                    [],
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
            },
        )?;
        let Some(last) = last_id else { break };
        after_id = last;
        // Let a concurrent writer in between pages (see `re_resolve_all_with_progress`).
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    phase.done = 1;
    progress(phase);
    Ok(PairStats { paired })
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: these assertions exercise the
// private SQL builders used verbatim by pairing. Duplicating the queries in
// an integration test could pass after production regressed.
#[path = "../tests/unit/scanner.rs"]
mod pairing_plan_tests;

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

/// Whether a listed path no longer exists at all, as opposed to existing
/// but failing to stat.
fn vanished(path: &Path) -> bool {
    crate::volume_io::symlink_metadata(path)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
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
    crate::index_store::upsert_issue_with_descriptor(
        conn,
        path.as_deref(),
        kind,
        Some(scan_issue_message_key(kind)),
        None,
        message,
    )
    .map(|_| ())
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
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

fn ensure_trailing_separator(path: &str) -> String {
    if path.ends_with(std::path::MAIN_SEPARATOR) {
        path.to_string()
    } else {
        format!("{path}{}", std::path::MAIN_SEPARATOR)
    }
}

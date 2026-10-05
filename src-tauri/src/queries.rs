//! Read-model queries for the UI. The unit everywhere is the LOGICAL file:
//! hashed rows collapse by content hash (a logical item's display time is the
//! earliest acceptable time after every live copy has completed date
//! checking), and unhashed rows (unique-size other-files, which by
//! construction have no duplicates) each stand alone.
//! Companions never appear — they ride with their primary.
//!
//! Month bucketing happens in Rust under the given display timezone, not in
//! SQL's UTC strftime: for a JST user, photos taken before 09:00 on the 1st
//! belong to the new month, and SQL's UTC month would misfile them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{Datelike, TimeZone};
use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MonthSection {
    /// `"2016-03"`, or `"undated"` for the trailing section.
    pub month: String,
    pub count: u64,
}

#[derive(Serialize, Debug, Default, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SectionCounts {
    pub images: Vec<MonthSection>,
    pub videos: Vec<MonthSection>,
    pub others: Vec<MonthSection>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct SectionLocation {
    pub kind: String,
    pub month: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTarget {
    pub identity: SectionIdentity,
    pub section: SectionLocation,
}

/// Diagnostic paths are lookup keys, never permission to discover new files.
pub fn resolve_library_path(
    conn: &Connection,
    path: &str,
    source_dirs: &[String],
    display_tz: Tz,
) -> Result<Option<LibraryTarget>, String> {
    let in_sources = |value: &str| {
        let path = Path::new(value);
        path.is_absolute()
            && !crate::trash::is_trash_path(path)
            && !path.components().any(|part| matches!(part, std::path::Component::ParentDir))
            && source_dirs.iter().any(|root| path.starts_with(root))
    };
    if !in_sources(path) { return Ok(None); }
    let row: Option<(i64, Option<String>, String)> = conn.query_row(
        "SELECT main.id, main.content_hash, main.abs_path FROM paths AS target
         JOIN paths AS main ON main.id = COALESCE(target.companion_of, target.id)
         WHERE target.abs_path = ?1 AND target.missing = 0
           AND main.missing = 0 AND main.companion_of IS NULL",
        [path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(|error| error.to_string())?;
    let Some((path_id, hash, main_path)) = row else { return Ok(None); };
    if !in_sources(&main_path) { return Ok(None); }
    let identity = SectionIdentity { hash, path_id };
    Ok(section_for_identity(conn, &identity, display_tz)?
        .map(|section| LibraryTarget { identity, section }))
}

/// Resolve one current logical identity, never its former ordinal or directory.
pub fn section_for_identity(
    conn: &Connection,
    identity: &SectionIdentity,
    display_tz: Tz,
) -> Result<Option<SectionLocation>, String> {
    let facts: Option<(String, Option<i64>)> = if let Some(hash) = &identity.hash {
        conn.query_row(
            "SELECT kind, resolved_utc_ms FROM review_contents WHERE content_hash = ?1 AND live_copy_count > 0",
            [hash], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()
    } else {
        conn.query_row(
            "SELECT 'other', resolved_utc_ms FROM paths WHERE id = ?1 AND missing = 0
             AND review_visible = 1 AND companion_of IS NULL AND content_hash IS NULL AND kind NOT IN ('image', 'video')",
            [identity.path_id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()
    }.map_err(|error| error.to_string())?;
    facts.map(|(kind, instant)| {
        let month = match instant {
            None => "undated".to_string(),
            Some(value) => {
                let local = display_tz.timestamp_millis_opt(value).single()
                    .ok_or_else(|| "Item date is outside the supported range".to_string())?;
                format!("{:04}-{:02}", local.year(), local.month())
            }
        };
        Ok(SectionLocation { kind, month })
    }).transpose()
}

const LOGICAL_MONTH_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM logical_contents INDEXED BY idx_logical_contents_section
     WHERE visible_copy_count > 0 AND kind = ?1 AND resolved_utc_ms >= ?2 AND resolved_utc_ms < ?3";
const UNHASHED_OTHER_MONTH_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM paths INDEXED BY idx_paths_unhashed_other_section
     WHERE missing = 0 AND review_visible = 1 AND companion_of IS NULL AND content_hash IS NULL
       AND kind NOT IN ('image', 'video')
       AND resolved_utc_ms >= ?1 AND resolved_utc_ms < ?2";
const LOGICAL_UNDATED_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM logical_contents INDEXED BY idx_logical_contents_section
     WHERE visible_copy_count > 0 AND kind = ?1 AND resolved_utc_ms IS NULL";
const UNHASHED_OTHER_UNDATED_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM paths INDEXED BY idx_paths_unhashed_other_section
     WHERE missing = 0 AND review_visible = 1 AND companion_of IS NULL AND content_hash IS NULL
       AND kind NOT IN ('image', 'video') AND resolved_utc_ms IS NULL";

/// Logical items per kind per month (oldest month first, Undated last).
pub fn section_counts(conn: &Connection, display_tz: Tz) -> Result<SectionCounts, String> {
    Ok(SectionCounts {
        images: sections_for_kind(conn, "image", display_tz)?,
        videos: sections_for_kind(conn, "video", display_tz)?,
        others: sections_for_kind(conn, "other", display_tz)?,
    })
}

fn logical_edge_sql(newest: bool) -> String {
    let direction = if newest { "DESC" } else { "ASC" };
    format!(
        "SELECT resolved_utc_ms
         FROM logical_contents INDEXED BY idx_logical_contents_section
         WHERE visible_copy_count > 0 AND kind = ?1 AND resolved_utc_ms IS NOT NULL
         ORDER BY resolved_utc_ms {direction} LIMIT 1"
    )
}

fn unhashed_other_edge_sql(newest: bool) -> String {
    let direction = if newest { "DESC" } else { "ASC" };
    format!(
        "SELECT resolved_utc_ms
         FROM paths INDEXED BY idx_paths_unhashed_other_section
         WHERE missing = 0 AND review_visible = 1 AND companion_of IS NULL AND content_hash IS NULL
           AND kind NOT IN ('image', 'video') AND resolved_utc_ms IS NOT NULL
         ORDER BY resolved_utc_ms {direction} LIMIT 1"
    )
}

fn optional_edge(conn: &Connection, sql: &str, kind: Option<&str>) -> Result<Option<i64>, String> {
    let result = match kind {
        Some(kind) => conn.query_row(sql, [kind], |row| row.get(0)).optional(),
        None => conn.query_row(sql, [], |row| row.get(0)).optional(),
    };
    result.map_err(|error| error.to_string())
}

fn sections_for_kind(
    conn: &Connection,
    kind: &str,
    display_tz: Tz,
) -> Result<Vec<MonthSection>, String> {
    let mut oldest = vec![optional_edge(conn, &logical_edge_sql(false), Some(kind))?];
    let mut newest = vec![optional_edge(conn, &logical_edge_sql(true), Some(kind))?];
    if kind == "other" {
        oldest.push(optional_edge(conn, &unhashed_other_edge_sql(false), None)?);
        newest.push(optional_edge(conn, &unhashed_other_edge_sql(true), None)?);
    }
    let oldest = oldest.into_iter().flatten().min();
    let newest = newest.into_iter().flatten().max();
    let mut sections = Vec::new();

    if let (Some(oldest), Some(newest)) = (oldest, newest) {
        let oldest = display_tz
            .timestamp_millis_opt(oldest)
            .earliest()
            .ok_or_else(|| "oldest resolved timestamp is outside the calendar".to_string())?;
        let newest = display_tz
            .timestamp_millis_opt(newest)
            .latest()
            .ok_or_else(|| "newest resolved timestamp is outside the calendar".to_string())?;
        let (mut year, mut month) = (oldest.year(), oldest.month());
        let last = (newest.year(), newest.month());
        let mut logical_count = conn
            .prepare(LOGICAL_MONTH_COUNT_SQL)
            .map_err(|error| error.to_string())?;
        let mut other_count = (kind == "other")
            .then(|| conn.prepare(UNHASHED_OTHER_MONTH_COUNT_SQL))
            .transpose()
            .map_err(|error| error.to_string())?;

        loop {
            let key = format!("{year:04}-{month:02}");
            let (start, end) = month_bounds(&key, display_tz)?
                .ok_or_else(|| format!("dated month unexpectedly has no bounds: {key}"))?;
            let mut count = logical_count
                .query_row(rusqlite::params![kind, start, end], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|error| error.to_string())?
                .max(0) as u64;
            if let Some(statement) = &mut other_count {
                count += statement
                    .query_row(rusqlite::params![start, end], |row| row.get::<_, i64>(0))
                    .map_err(|error| error.to_string())?
                    .max(0) as u64;
            }
            if count > 0 {
                sections.push(MonthSection { month: key, count });
            }
            if (year, month) == last {
                break;
            }
            if month == 12 {
                year += 1;
                month = 1;
            } else {
                month += 1;
            }
        }
    }

    let mut undated = conn
        .query_row(LOGICAL_UNDATED_COUNT_SQL, [kind], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| error.to_string())?
        .max(0) as u64;
    if kind == "other" {
        undated += conn
            .query_row(UNHASHED_OTHER_UNDATED_COUNT_SQL, [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| error.to_string())?
            .max(0) as u64;
    }
    if undated > 0 {
        sections.push(MonthSection {
            month: "undated".to_string(),
            count: undated,
        });
    }
    Ok(sections)
}

struct CachedSectionCounts {
    data_version: i64,
    display_tz: Tz,
    counts: SectionCounts,
}

struct SectionCountsCache {
    db_file: PathBuf,
    conn: Connection,
    cached: Option<CachedSectionCounts>,
}

impl SectionCountsCache {
    fn open(db_file: &Path) -> Result<Self, String> {
        Ok(Self {
            db_file: db_file.to_path_buf(),
            conn: crate::index_store::open(db_file)?,
            cached: None,
        })
    }

    fn load(&mut self, display_tz: Tz) -> Result<(SectionCounts, bool), String> {
        let data_version = self
            .conn
            .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
            .map_err(|error| error.to_string())?;
        if let Some(cached) = &self.cached {
            if cached.data_version == data_version && cached.display_tz == display_tz {
                return Ok((cached.counts.clone(), false));
            }
        }

        let transaction = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Deferred,
        )
        .map_err(|error| error.to_string())?;
        let counts = section_counts(&transaction, display_tz)?;
        transaction.commit().map_err(|error| error.to_string())?;
        self.cached = Some(CachedSectionCounts {
            data_version,
            display_tz,
            counts: counts.clone(),
        });
        Ok((counts, true))
    }
}

static SECTION_COUNTS_CACHE: Mutex<Option<SectionCountsCache>> = Mutex::new(None);

/// Reuses the exact count projection while SQLite reports the same committed
/// index snapshot and the OS display timezone is unchanged. The cache owns a
/// read-only-in-practice observer connection, so `PRAGMA data_version` changes
/// for every writer connection without adding a revision table or trigger.
///
/// The global mutex is held only to take and to return the cache slot, never
/// while `load` recomputes: a scan-time pile-up of concurrent callers used to
/// queue on this lock for the full O(library) recount instead of each running
/// (redundantly, on a miss) in parallel. Every caller reunites its slot with
/// the shared one afterward so the persistent connection is normally reused.
pub fn cached_section_counts(db_file: &Path, display_tz: Tz) -> Result<SectionCounts, String> {
    let mut owned = {
        let mut slot = SECTION_COUNTS_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match slot.take() {
            Some(existing) if existing.db_file == db_file => existing,
            _ => SectionCountsCache::open(db_file)?,
        }
    };

    let result = owned.load(display_tz).map(|(counts, _)| counts);

    let mut slot = SECTION_COUNTS_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some(owned);
    result
}

/// One grid row: a logical file within a section. `hash` is None for
/// unhashed unique-size other-files (their identity is the representative
/// path itself).
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SectionItem {
    pub hash: Option<String>,
    pub path_id: i64,
    pub file_name: String,
    pub resolved_utc_ms: Option<i64>,
    pub copy_count: u64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub has_thumb: bool,
    pub similar_group_id: Option<i64>,
    pub similar_count: u64,
    pub sharpness: Option<f64>,
    /// Ready face score for advisory presentation; None remains unscored or
    /// failed, while zero is a successful no-face result.
    pub face_score: Option<f64>,
    pub byte_size: Option<i64>,
    pub has_companions: bool,
    pub duration_ms: Option<i64>,
    /// EVERY live copy's directory, deduped, sorted, display-stripped
    /// (`for_display`, like copy_paths). The other-files table shows them
    /// all in one Folders column — copies merge into one row, so a single
    /// representative folder was an arbitrary MIN and sorting by it was
    /// meaningless (Phase 33 dropped folder sort with it).
    pub dir_paths: Vec<String>,
    pub derived_work: crate::derived_state::ItemWorkStates,
}

/// The OS display timezone that section months are bucketed in.
pub fn display_timezone() -> chrono_tz::Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

/// The three Main sections. Deserialized once at the command boundary, so
/// every reader below works with a kind that is already valid.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum SectionKind {
    Image,
    Video,
    Other,
}

impl SectionKind {
    /// The section's `logical_contents.kind`; audio and every other
    /// non-image, non-video kind project to Other.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Other => "other",
        }
    }
}

impl rusqlite::ToSql for SectionKind {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

#[derive(Clone, Copy)]
pub struct ItemProjectionContext {
    pub capabilities: crate::derived_state::WorkCapabilities,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SectionSortOrder {
    Time,
    Name,
    Size,
    Resolution,
    Ext,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SectionSort {
    pub order: SectionSortOrder,
    pub desc: bool,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SectionWindow {
    pub total: u64,
    pub start: u64,
    pub items: Vec<SectionItem>,
    /// Identifies the complete section order this window was sliced from.
    /// Positions learned under one order token are valid only while a later
    /// window reports the same token.
    pub order: String,
}

pub const MAX_SECTION_WINDOW_ITEMS: u32 = 512;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct SectionIdentity {
    pub hash: Option<String>,
    pub path_id: i64,
}

impl SectionIdentity {
    fn key(&self) -> String {
        identity_key(self.hash.as_deref(), self.path_id)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PositionedSectionIdentity {
    pub hash: Option<String>,
    pub path_id: i64,
    pub index: u64,
}

impl PositionedSectionIdentity {
    fn from_identity(identity: SectionIdentity, index: u64) -> Self {
        Self {
            hash: identity.hash,
            path_id: identity.path_id,
            index,
        }
    }

    fn key(&self) -> String {
        identity_key(self.hash.as_deref(), self.path_id)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionRecoveryContext {
    pub index: u64,
    pub before: Vec<SectionIdentity>,
    pub after: Vec<SectionIdentity>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SectionReconciliation {
    pub anchor: Option<PositionedSectionIdentity>,
    pub selected: Vec<PositionedSectionIdentity>,
    pub range_origin: Option<PositionedSectionIdentity>,
    pub range_base: Vec<PositionedSectionIdentity>,
    pub context: Option<SectionRecoveryContextOutput>,
    pub window: SectionWindow,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SectionRecoveryContextOutput {
    pub index: u64,
    pub before: Vec<SectionIdentity>,
    pub after: Vec<SectionIdentity>,
}

/// A bounded ordered window into one section. SQLite owns ordering and may
/// spill a large non-index sort to its temp store; the webview never receives
/// or retains rows outside the requested window.
pub fn section_window(
    conn: &Connection,
    kind: SectionKind,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    start: u64,
    limit: u32,
    projection: ItemProjectionContext,
) -> Result<SectionWindow, String> {
    let snapshot = SectionSnapshot::begin(conn)?;
    let window = section_window_snapshot(
        &snapshot, kind.as_str(), month, display_tz, sort, start, limit, projection,
    )?;
    snapshot.finish()?;
    Ok(window)
}

#[derive(Clone)]
struct SectionOrderCache {
    kind: String,
    month: String,
    sort: SectionSort,
    revision: i64,
    identities: Arc<SectionOrder>,
}

/// One section's complete ordered identity sequence and a token that changes
/// exactly when that sequence does (membership, order, or an identity), so a
/// caller holding positions from an earlier read can tell whether they still
/// hold without resending them.
pub(crate) struct SectionOrder {
    identities: Vec<SectionIdentity>,
    token: String,
}

impl SectionOrder {
    fn new(identities: Vec<SectionIdentity>) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        identities.hash(&mut hasher);
        let token = format!("{:016x}", hasher.finish());
        Self { identities, token }
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

impl std::ops::Deref for SectionOrder {
    type Target = [SectionIdentity];

    fn deref(&self) -> &[SectionIdentity] {
        &self.identities
    }
}

/// One cached section order per index file, keyed by its path.
static SECTION_ORDER_CACHE: std::sync::LazyLock<Mutex<HashMap<String, SectionOrderCache>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// One long-lived observer connection per index file, keyed by its path. An
/// observer is never replaced: a replacement's `data_version` would not be
/// comparable with its predecessor's.
static INDEX_REVISION_OBSERVERS: std::sync::LazyLock<Mutex<HashMap<String, Connection>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// The committed index revision, comparable across every connection to
/// `db_file`. `PRAGMA data_version` is only comparable between two reads on
/// the same connection and only moves for commits made by *other*
/// connections, so one long-lived observer connection that never writes owns
/// it: every commit by any command, worker or watcher connection advances it.
/// A fresh per-command connection's own value never changes and must not be
/// used as a revision.
fn observed_index_revision(db_file: &str) -> Result<i64, String> {
    let mut observers = INDEX_REVISION_OBSERVERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !observers.contains_key(db_file) {
        let conn = crate::index_store::open(Path::new(db_file))?;
        observers.insert(db_file.to_string(), conn);
    }
    observers[db_file]
        .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
        .map_err(|error| error.to_string())
}

/// One read snapshot of the index for a section query, and the index revision
/// that snapshot holds when it is known exactly.
///
/// The observer revision is read immediately before and after this snapshot
/// is pinned. Equal reads prove no commit landed in between, so the snapshot
/// holds exactly that revision and may share the cached section order. When
/// they differ, or when the caller already holds an open transaction whose
/// snapshot may predate both reads, the order is computed from this snapshot
/// and not cached; reusing an order from another snapshot could name an item
/// this snapshot cannot load, or miss one it can.
struct SectionSnapshot<'conn> {
    conn: &'conn Connection,
    transaction: Option<rusqlite::Transaction<'conn>>,
    revision: Option<i64>,
}

impl<'conn> SectionSnapshot<'conn> {
    fn begin(conn: &'conn Connection) -> Result<Self, String> {
        let db_file = conn.path().filter(|path| !path.is_empty()).map(str::to_string);
        if !conn.is_autocommit() || db_file.is_none() {
            return Ok(Self {
                conn,
                transaction: None,
                revision: None,
            });
        }
        let db_file = db_file.expect("checked above");
        let before = observed_index_revision(&db_file)?;
        let transaction =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
                .map_err(|error| error.to_string())?;
        transaction
            .query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| error.to_string())?;
        let after = observed_index_revision(&db_file)?;
        Ok(Self {
            conn,
            transaction: Some(transaction),
            revision: (before == after).then_some(before),
        })
    }

    fn finish(self) -> Result<(), String> {
        match self.transaction {
            Some(transaction) => transaction.commit().map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }
}

impl std::ops::Deref for SectionSnapshot<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn
    }
}

/// Returns one section's complete ordered identity sequence, sorted once per
/// index revision instead of once per caller. `reconcile_section`, section
/// windows, range reads, family-context recovery, and viewer-session
/// materialization share this one ordering for a given `(db, kind, month,
/// sort)` while the index revision stays the same.
fn ordered_section_identities(
    snapshot: &SectionSnapshot<'_>,
    kind: &str,
    month: &str,
    bounds: Option<(i64, i64)>,
    sort: SectionSort,
) -> Result<Arc<SectionOrder>, String> {
    let db_file = snapshot.path().unwrap_or_default().to_string();
    if let Some(revision) = snapshot.revision {
        let cache = SECTION_ORDER_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = cache.get(&db_file) {
            if existing.kind == kind
                && existing.month == month
                && existing.sort == sort
                && existing.revision == revision
            {
                return Ok(existing.identities.clone());
            }
        }
    }

    let candidates = section_candidates_sql(kind == "other", bounds.is_some());
    let params = section_candidate_params(kind, bounds);
    let sql = format!(
        "WITH candidates AS ({candidates}) SELECT hash, path_id FROM candidates ORDER BY {}",
        section_order_sql(sort)
    );
    let mut statement = snapshot.prepare(&sql).map_err(|error| error.to_string())?;
    let identities = statement
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(SectionIdentity {
                hash: row.get(0)?,
                path_id: row.get(1)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    drop(statement);
    let identities = Arc::new(SectionOrder::new(identities));

    if let Some(revision) = snapshot.revision {
        let mut cache = SECTION_ORDER_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert(db_file, SectionOrderCache {
            kind: kind.to_string(),
            month: month.to_string(),
            sort,
            revision,
            identities: identities.clone(),
        });
    }
    Ok(identities)
}

/// Clamps `[start, end)` to `ordered`'s bounds and slices it without copying.
fn ordered_section_slice(ordered: &[SectionIdentity], start: u64, end: u64) -> &[SectionIdentity] {
    let total = ordered.len() as u64;
    let start = start.min(total) as usize;
    let end = end.min(total).max(start as u64) as usize;
    &ordered[start..end]
}

fn recovery_context_from_ordered(
    ordered: &[SectionIdentity],
    index: u64,
) -> SectionRecoveryContextOutput {
    let start = index.saturating_sub(RECOVERY_NEIGHBOR_LIMIT);
    let end = index.saturating_add(RECOVERY_NEIGHBOR_LIMIT + 1);
    let slice = ordered_section_slice(ordered, start, end);
    let anchor_offset = ((index - start) as usize).min(slice.len());
    SectionRecoveryContextOutput {
        index,
        before: slice[..anchor_offset].iter().rev().cloned().collect(),
        after: slice.get(anchor_offset + 1..).unwrap_or_default().to_vec(),
    }
}

#[allow(clippy::too_many_arguments)]
fn section_window_snapshot(
    snapshot: &SectionSnapshot<'_>,
    kind: &str,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    start: u64,
    limit: u32,
    projection: ItemProjectionContext,
) -> Result<SectionWindow, String> {
    if !(1..=MAX_SECTION_WINDOW_ITEMS).contains(&limit) {
        return Err(format!(
            "section window limit must be between 1 and {MAX_SECTION_WINDOW_ITEMS}"
        ));
    }
    let bounds = month_bounds(month, display_tz)?;
    let ordered = ordered_section_identities(snapshot, kind, month, bounds, sort)?;
    let total = ordered.len() as u64;
    let start = start.min(total);
    if start == total {
        return Ok(SectionWindow {
            total,
            start,
            items: Vec::new(),
            order: ordered.token().to_string(),
        });
    }
    let end = start.saturating_add(u64::from(limit));
    let identities = ordered_section_slice(&ordered, start, end);
    Ok(SectionWindow {
        total,
        start,
        items: section_items_by_identity(snapshot, identities, projection)?,
        order: ordered.token().to_string(),
    })
}

const RECOVERY_NEIGHBOR_LIMIT: u64 = 64;

/// Reconciles selection and the work-position anchor against one ordered
/// section snapshot, then returns only the display window around the chosen
/// anchor. The section is sorted exactly once (`ordered_section_identities`);
/// the match scan, display window, and recovery neighbors are all sliced from
/// that single ordering instead of each re-sorting the section.
#[allow(clippy::too_many_arguments)]
pub fn reconcile_section(
    conn: &Connection,
    kind: SectionKind,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    selected: &[PositionedSectionIdentity],
    anchor: Option<&SectionIdentity>,
    range_origin: Option<&SectionIdentity>,
    range_base: &[SectionIdentity],
    recovery: Option<&SectionRecoveryContext>,
    select_first: bool,
    limit: u32,
    projection: ItemProjectionContext,
) -> Result<SectionReconciliation, String> {
    let kind = kind.as_str();
    if !(1..=MAX_SECTION_WINDOW_ITEMS).contains(&limit) {
        return Err(format!(
            "section window limit must be between 1 and {MAX_SECTION_WINDOW_ITEMS}"
        ));
    }
    let bounds = month_bounds(month, display_tz)?;
    let snapshot = SectionSnapshot::begin(conn)?;
    let ordered = ordered_section_identities(&snapshot, kind, month, bounds, sort)?;
    let total = ordered.len() as u64;

    let mut wanted: HashSet<String> = anchor
        .into_iter()
        .chain(range_origin)
        .chain(range_base)
        .chain(
            recovery
                .into_iter()
                .flat_map(|context| context.before.iter().chain(context.after.iter())),
        )
        .map(SectionIdentity::key)
        .collect();
    wanted.extend(selected.iter().map(PositionedSectionIdentity::key));
    let mut matches = HashMap::<String, PositionedSectionIdentity>::with_capacity(wanted.len());
    for (index, identity) in ordered.iter().enumerate() {
        let key = identity.key();
        if wanted.contains(&key) {
            matches.insert(
                key,
                PositionedSectionIdentity::from_identity(identity.clone(), index as u64),
            );
        }
    }

    // The caller's own former window position for each selected identity,
    // in the same before-the-refresh coordinate space as a recovery
    // context's `index`. `matches` only carries *current* positions, which
    // shift under removal and cannot be compared against a former position
    // (R5.1 D5).
    let selected_former_index: HashMap<String, u64> = selected
        .iter()
        .map(|identity| (identity.key(), identity.index))
        .collect();
    let mut live_selected = matched_identities(selected, &matches);
    live_selected.sort_by_key(|member| member.index);
    let mut live_range_base = matched_identities(range_base, &matches);
    live_range_base.sort_by_key(|member| member.index);
    let live_range_origin = range_origin.and_then(|identity| matches.get(&identity.key()).cloned());
    let chosen = choose_reconciled_anchor(
        anchor,
        recovery,
        &matches,
        &live_selected,
        &selected_former_index,
        select_first,
        total,
    );
    let anchor = match chosen {
        Some(AnchorChoice::Known(member)) => Some(member),
        Some(AnchorChoice::Index(index)) => ordered
            .get(index as usize)
            .map(|identity| PositionedSectionIdentity::from_identity(identity.clone(), index)),
        None => None,
    };

    let anchor_index = anchor.as_ref().map_or(0, |member| member.index);
    let max_start = total.saturating_sub(u64::from(limit));
    let start = anchor_index
        .saturating_sub(u64::from(limit) / 2)
        .min(max_start);
    let end = start.saturating_add(u64::from(limit));
    let window = SectionWindow {
        total,
        start,
        items: section_items_by_identity(
            &snapshot,
            ordered_section_slice(&ordered, start, end),
            projection,
        )?,
        order: ordered.token().to_string(),
    };
    snapshot.finish()?;
    let context = anchor
        .as_ref()
        .map(|member| recovery_context_from_ordered(&ordered, member.index));

    Ok(SectionReconciliation {
        anchor,
        selected: live_selected,
        range_origin: live_range_origin,
        range_base: live_range_base,
        context,
        window,
    })
}

/// Shared by `SectionIdentity` and `PositionedSectionIdentity` so
/// `matched_identities` can resolve either kind of request against the
/// current ordered section.
trait Keyed {
    fn key(&self) -> String;
}

impl Keyed for SectionIdentity {
    fn key(&self) -> String {
        SectionIdentity::key(self)
    }
}

impl Keyed for PositionedSectionIdentity {
    fn key(&self) -> String {
        PositionedSectionIdentity::key(self)
    }
}

fn matched_identities<T: Keyed>(
    requested: &[T],
    matches: &HashMap<String, PositionedSectionIdentity>,
) -> Vec<PositionedSectionIdentity> {
    requested
        .iter()
        .filter_map(|identity| matches.get(&identity.key()).cloned())
        .collect()
}

enum AnchorChoice {
    Known(PositionedSectionIdentity),
    Index(u64),
}

fn choose_reconciled_anchor(
    requested: Option<&SectionIdentity>,
    recovery: Option<&SectionRecoveryContext>,
    matches: &HashMap<String, PositionedSectionIdentity>,
    live_selected: &[PositionedSectionIdentity],
    selected_former_index: &HashMap<String, u64>,
    select_first: bool,
    total: u64,
) -> Option<AnchorChoice> {
    if total == 0 {
        return None;
    }
    let Some(requested) = requested else {
        return select_first.then_some(AnchorChoice::Index(0));
    };
    if let Some(member) = matches.get(&requested.key()) {
        return Some(AnchorChoice::Known(member.clone()));
    }

    let selected_keys: HashSet<String> = live_selected
        .iter()
        .map(PositionedSectionIdentity::key)
        .collect();
    if let Some(context) = recovery {
        let selected_neighbor = context
            .after
            .iter()
            .chain(context.before.iter())
            .filter(|identity| selected_keys.contains(&identity.key()))
            .find_map(|identity| matches.get(&identity.key()).cloned());
        if let Some(member) = selected_neighbor {
            return Some(AnchorChoice::Known(member));
        }
        // Compare former positions on both sides -- the anchor's own former
        // index (`context.index`) and each survivor's former index -- rather
        // than a survivor's current index, which shifts under removal and is
        // not comparable to `context.index` (R5.1 D5).
        if let Some(member) = live_selected
            .iter()
            .find(|member| {
                selected_former_index
                    .get(&member.key())
                    .is_some_and(|&former| former >= context.index)
            })
            .or_else(|| live_selected.last())
        {
            return Some(AnchorChoice::Known(member.clone()));
        }
        let neighbor = context
            .after
            .iter()
            .chain(context.before.iter())
            .find_map(|identity| matches.get(&identity.key()).cloned());
        if let Some(member) = neighbor {
            return Some(AnchorChoice::Known(member));
        }
        return Some(AnchorChoice::Index(context.index.min(total - 1)));
    }
    Some(AnchorChoice::Index(0))
}

/// Returns the ordered identities for one deliberate Main range-selection.
/// The result may be large because its size is the user's explicit selection,
/// while ordinary browsing and rendering remain capped.
pub fn section_range(
    conn: &Connection,
    kind: SectionKind,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    start: u64,
    end: u64,
) -> Result<Vec<PositionedSectionIdentity>, String> {
    let kind = kind.as_str();
    let bounds = month_bounds(month, display_tz)?;
    let snapshot = SectionSnapshot::begin(conn)?;
    let ordered = ordered_section_identities(&snapshot, kind, month, bounds, sort)?;
    snapshot.finish()?;
    let members = ordered_section_slice(&ordered, start, end)
        .iter()
        .enumerate()
        .map(|(offset, identity)| {
            PositionedSectionIdentity::from_identity(identity.clone(), start + offset as u64)
        })
        .collect();
    Ok(members)
}

/// Captures the bounded Main recovery neighborhood after the last member of
/// a Comparison family in the current order. Comparison keeps this before it
/// changes files, so Main can return past the original family afterward.
pub fn section_family_context(
    conn: &Connection,
    kind: SectionKind,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    member_hashes: &[String],
) -> Result<Option<SectionRecoveryContextOutput>, String> {
    let kind = kind.as_str();
    let bounds = month_bounds(month, display_tz)?;
    let family: HashSet<&str> = member_hashes.iter().map(String::as_str).collect();
    let snapshot = SectionSnapshot::begin(conn)?;
    let ordered = ordered_section_identities(&snapshot, kind, month, bounds, sort)?;
    snapshot.finish()?;
    let last_member = ordered
        .iter()
        .enumerate()
        .filter(|(_, identity)| {
            identity
                .hash
                .as_deref()
                .is_some_and(|value| family.contains(value))
        })
        .map(|(index, _)| index as u64)
        .last();
    Ok(last_member.map(|index| recovery_context_from_ordered(&ordered, index)))
}

/// Visits one complete ordered identity sequence from the shared per-revision
/// ordering. The caller may persist it in a disposable store while relying on
/// the same section order every other reader currently sees.
pub fn visit_section_identities(
    conn: &Connection,
    kind: SectionKind,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    mut visit: impl FnMut(u64, &SectionIdentity) -> Result<(), String>,
) -> Result<u64, String> {
    let kind = kind.as_str();
    let bounds = month_bounds(month, display_tz)?;
    let snapshot = SectionSnapshot::begin(conn)?;
    let ordered = ordered_section_identities(&snapshot, kind, month, bounds, sort)?;
    snapshot.finish()?;
    for (index, identity) in ordered.iter().enumerate() {
        visit(index as u64, identity)?;
    }
    Ok(ordered.len() as u64)
}

fn section_candidate_params(kind: &str, bounds: Option<(i64, i64)>) -> Vec<rusqlite::types::Value> {
    let mut values = vec![kind.to_string().into()];
    if let Some((start, end)) = bounds {
        values.push(start.into());
        values.push(end.into());
    }
    values
}

fn section_candidates_sql(include_unhashed_other: bool, has_bounds: bool) -> String {
    let logical_time = if has_bounds {
        "l.resolved_utc_ms >= ?2 AND l.resolved_utc_ms < ?3"
    } else {
        "l.resolved_utc_ms IS NULL"
    };
    let mut sql = format!(
        "SELECT c.hash AS hash, l.representative_path_id AS path_id, \
                rp.file_name AS file_name, l.resolved_utc_ms AS resolved_utc_ms, \
                c.byte_size AS byte_size, c.width AS width, c.height AS height, \
                lower(rp.ext) AS extension \
         FROM review_contents l \
         JOIN contents c ON c.hash = l.content_hash \
         JOIN paths rp ON rp.id = l.representative_path_id \
         WHERE l.kind = ?1 AND {logical_time}"
    );
    if include_unhashed_other {
        let other_time = if has_bounds {
            "p.resolved_utc_ms >= ?2 AND p.resolved_utc_ms < ?3"
        } else {
            "p.resolved_utc_ms IS NULL"
        };
        sql.push_str(&format!(
            " UNION ALL \
             SELECT NULL, p.id, p.file_name, p.resolved_utc_ms, p.size, \
                    NULL, NULL, lower(p.ext) \
             FROM paths p \
             WHERE p.missing = 0 AND p.review_visible = 1 AND p.companion_of IS NULL \
               AND p.content_hash IS NULL AND p.kind NOT IN ('image', 'video') \
               AND {other_time}"
        ));
    }
    sql
}

fn section_order_terms(sort: SectionSort) -> Vec<(&'static str, bool)> {
    let time = [
        ("resolved_utc_ms IS NULL", false),
        ("COALESCE(resolved_utc_ms, 0)", false),
    ];
    let name = ("file_name COLLATE onecopy_nocase", false);
    let mut terms = match sort.order {
        SectionSortOrder::Time => vec![(time[0].0, sort.desc), (time[1].0, sort.desc), name],
        SectionSortOrder::Name => vec![(name.0, sort.desc), time[0], time[1]],
        SectionSortOrder::Size => vec![
            ("COALESCE(byte_size, -1)", sort.desc),
            time[0],
            time[1],
            name,
        ],
        SectionSortOrder::Resolution => vec![
            ("(COALESCE(width, 0) * COALESCE(height, 0))", sort.desc),
            time[0],
            time[1],
            name,
        ],
        SectionSortOrder::Ext => vec![("COALESCE(extension, '')", sort.desc), name],
    };
    terms.push(("path_id", false));
    terms
}

fn section_order_sql(sort: SectionSort) -> String {
    section_order_terms(sort)
        .iter()
        .map(|(term, desc)| format!("{term} {}", if *desc { "DESC" } else { "ASC" }))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Captured sort values remain usable if a completed item disappears between turns.
#[derive(Clone)]
pub struct SectionWorkPosition {
    pub hash: Option<String>,
    values: Vec<rusqlite::types::Value>,
}

pub fn section_work_anchor(
    conn: &Connection,
    kind: SectionKind,
    bounds: Option<(i64, i64)>,
    sort: SectionSort,
    index: u64,
) -> Result<Option<SectionWorkPosition>, String> {
    section_work_rows(conn, kind.as_str(), bounds, sort, None, false, 1, index, None)
        .map(|mut rows| rows.pop())
}

pub fn section_work_page(
    conn: &Connection,
    kind: SectionKind,
    bounds: Option<(i64, i64)>,
    sort: SectionSort,
    position: &SectionWorkPosition,
    before: bool,
) -> Result<Vec<SectionWorkPosition>, String> {
    section_work_rows(
        conn,
        kind.as_str(),
        bounds,
        sort,
        Some(position),
        before,
        64,
        0,
        None,
    )
}

pub(crate) fn section_pending_work_page(
    conn: &Connection,
    kind: SectionKind,
    bounds: Option<(i64, i64)>,
    sort: SectionSort,
    position: &SectionWorkPosition,
    before: bool,
    pending: &str,
) -> Result<Vec<SectionWorkPosition>, String> {
    section_work_rows(
        conn,
        kind.as_str(),
        bounds,
        sort,
        Some(position),
        before,
        64,
        0,
        Some(pending),
    )
}

#[allow(clippy::too_many_arguments)]
fn section_work_rows(
    conn: &Connection,
    kind: &str,
    bounds: Option<(i64, i64)>,
    sort: SectionSort,
    position: Option<&SectionWorkPosition>,
    before: bool,
    limit: u64,
    offset: u64,
    pending: Option<&str>,
) -> Result<Vec<SectionWorkPosition>, String> {
    let terms = section_order_terms(sort);
    let candidates = section_candidates_sql(kind == "other", bounds.is_some());
    let mut params = section_candidate_params(kind, bounds);
    let mut branches = Vec::new();
    if let Some(position) = position {
        let base = params.len() + 1;
        params.extend(position.values.iter().cloned());
        for (index, (term, desc)) in terms.iter().enumerate() {
            let mut predicates = terms[..index]
                .iter()
                .enumerate()
                .map(|(prior, (term, _))| format!("({term}) IS ?{}", base + prior))
                .collect::<Vec<_>>();
            let comparator = if *desc ^ before { "<" } else { ">" };
            predicates.push(format!("({term}) {comparator} ?{}", base + index));
            branches.push(format!("({})", predicates.join(" AND ")));
        }
    }
    let mut filters = Vec::new();
    if !branches.is_empty() {
        filters.push(format!("({})", branches.join(" OR ")));
    }
    if let Some(pending) = pending {
        filters.push(format!(
            "EXISTS (SELECT 1 FROM review_contents l \
            JOIN contents c ON c.hash = l.content_hash \
            WHERE l.content_hash = candidates.hash AND ({pending}))"
        ));
    }
    let filter = if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    };
    let columns = terms
        .iter()
        .map(|(term, _)| *term)
        .collect::<Vec<_>>()
        .join(", ");
    let order = terms
        .iter()
        .map(|(term, desc)| format!("{term} {}", if *desc ^ before { "DESC" } else { "ASC" }))
        .collect::<Vec<_>>()
        .join(", ");
    params.push((limit as i64).into());
    params.push((offset.min(i64::MAX as u64) as i64).into());
    let sql = format!("WITH candidates AS ({candidates}) SELECT hash, {columns} FROM candidates {filter} ORDER BY {order} LIMIT ?{} OFFSET ?{}", params.len() - 1, params.len());
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(params), |row| {
            Ok(SectionWorkPosition {
                hash: row.get(0)?,
                values: (1..=terms.len())
                    .map(|index| row.get(index))
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string());
    rows
}

fn identity_key(hash: Option<&str>, path_id: i64) -> String {
    crate::indexed_file::item_key(hash, path_id)
}

fn section_items_by_identity(
    conn: &Connection,
    identities: &[SectionIdentity],
    projection: ItemProjectionContext,
) -> Result<Vec<SectionItem>, String> {
    let hashes: Vec<&str> = identities
        .iter()
        .filter_map(|identity| identity.hash.as_deref())
        .collect();
    let path_ids: Vec<i64> = identities
        .iter()
        .filter(|identity| identity.hash.is_none())
        .map(|identity| identity.path_id)
        .collect();
    let mut items = HashMap::with_capacity(identities.len());

    if !hashes.is_empty() {
        let placeholders = std::iter::repeat_n("?", hashes.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "{} WHERE c.hash IN ({placeholders})",
            hashed_section_select()
        );
        let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
        let hashed = statement
            .query_map(rusqlite::params_from_iter(hashes.iter()), |row| {
                section_item_from_row(row, projection)
            })
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
        drop(statement);
        let dirs = hashed_dirs_for_hashes(conn, &hashes)?;
        for mut item in hashed {
            if let Some(hash) = item.hash.clone() {
                item.dir_paths = dirs.get(&hash).cloned().unwrap_or_default();
                items.insert(hash, item);
            }
        }
    }

    if !path_ids.is_empty() {
        let placeholders = std::iter::repeat_n("?", path_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, file_name, resolved_utc_ms, size, dir_path FROM paths \
             WHERE id IN ({placeholders}) AND missing = 0 AND review_visible = 1 AND companion_of IS NULL \
               AND content_hash IS NULL AND kind NOT IN ('image', 'video')"
        );
        let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(path_ids.iter()), |row| {
                unhashed_other_item_from_row(row, projection)
            })
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
        for item in rows {
            items.insert(identity_key(None, item.path_id), item);
        }
    }

    identities
        .iter()
        .map(|identity| {
            items
                .remove(&identity_key(identity.hash.as_deref(), identity.path_id))
                .ok_or_else(|| "section item disappeared while its window was loading".to_string())
        })
        .collect()
}

fn hashed_dirs_for_hashes(
    conn: &Connection,
    hashes: &[&str],
) -> Result<HashMap<String, Vec<String>>, String> {
    let placeholders = std::iter::repeat_n("?", hashes.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT content_hash, dir_path FROM paths \
         WHERE content_hash IN ({placeholders}) AND missing = 0 \
           AND companion_of IS NULL ORDER BY content_hash, dir_path"
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            rusqlite::params_from_iter(hashes.iter()),
            section_dir_from_row,
        )
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    let mut by_hash: HashMap<String, Vec<String>> = HashMap::new();
    for (hash, dir) in rows {
        by_hash
            .entry(hash)
            .or_default()
            .push(crate::winpath::for_display(&dir).into_owned());
    }
    Ok(by_hash)
}

fn unhashed_other_item_from_row(
    row: &rusqlite::Row<'_>,
    projection: ItemProjectionContext,
) -> rusqlite::Result<SectionItem> {
    let dir: String = row.get(4)?;
    Ok(SectionItem {
        hash: None,
        path_id: row.get(0)?,
        file_name: row.get(1)?,
        resolved_utc_ms: row.get(2)?,
        copy_count: 1,
        width: None,
        height: None,
        has_thumb: false,
        similar_group_id: None,
        similar_count: 0,
        sharpness: None,
        face_score: None,
        byte_size: row.get(3)?,
        has_companions: false,
        duration_ms: None,
        dir_paths: vec![crate::winpath::for_display(&dir).into_owned()],
        derived_work: crate::derived_state::item_work_states(
            crate::derived_state::ItemWorkFacts {
                kind: "other",
                derived_at: None,
                derive_outcome: None,
                derived_version: 0,
                strip_frames: None,
                duration_ms: None,
                similar_group_id: None,
                face_state: None,
                face_score: None,
                transcript_state: None,
            },
            projection.capabilities,
            false,
        ),
    })
}

/// One logical row after a derived output changes. The coordinator publishes
/// this directly so the open grid patches one item instead of re-reading a
/// section that may contain millions of rows.
pub fn item_by_hash(
    conn: &Connection,
    hash: &str,
    projection: ItemProjectionContext,
) -> Result<Option<SectionItem>, String> {
    let sql = format!("{} WHERE c.hash = ?1", hashed_section_select());
    let mut item = conn
        .query_row(&sql, [hash], |row| section_item_from_row(row, projection))
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(item) = &mut item {
        let mut statement = conn
            .prepare(
                "SELECT DISTINCT dir_path FROM paths
                 WHERE content_hash = ?1 AND missing = 0 ORDER BY dir_path",
            )
            .map_err(|error| error.to_string())?;
        item.dir_paths = statement
            .query_map([hash], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
    }
    Ok(item)
}

pub fn item_by_identity(
    conn: &Connection,
    identity: &SectionIdentity,
    projection: ItemProjectionContext,
) -> Result<Option<SectionItem>, String> {
    if let Some(hash) = identity.hash.as_deref() {
        return item_by_hash(conn, hash, projection);
    }
    conn.query_row(
        "SELECT id, file_name, resolved_utc_ms, size, dir_path FROM paths \
         WHERE id = ?1 AND missing = 0 AND review_visible = 1 AND companion_of IS NULL \
           AND content_hash IS NULL AND kind NOT IN ('image', 'video')",
        [identity.path_id],
        |row| unhashed_other_item_from_row(row, projection),
    )
    .optional()
    .map_err(|error| error.to_string())
}

/// True only when every selected image currently belongs to the same live
/// similarity group. Main may retain selected identities outside its loaded
/// window, so Comparison admission cannot be inferred from display rows.
pub fn comparison_selection_valid(
    conn: &Connection,
    hashes: &[String],
) -> Result<bool, String> {
    if hashes.is_empty() {
        return Ok(false);
    }
    let mut statement = conn
        .prepare(
            "SELECT m.group_id FROM similar_group_members m \
             JOIN review_contents l ON l.content_hash = m.content_hash \
             WHERE m.content_hash = ?1 AND l.live_copy_count > 0 LIMIT 1",
        )
        .map_err(|error| error.to_string())?;
    let mut group = None;
    for hash in hashes {
        let current = statement
            .query_row([hash], |row| row.get::<_, i64>(0))
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(current) = current else {
            return Ok(false);
        };
        if group.is_some_and(|expected| expected != current) {
            return Ok(false);
        }
        group = Some(current);
    }
    Ok(true)
}

fn hashed_section_select() -> String {
    let preview_available = crate::derived_state::preview_available_predicate("c");
    let face_state = crate::derived_state::face_state_sql("c");
    let face_score = crate::derived_state::face_score_sql("c");
    let transcript_state = crate::derived_state::transcript_state_sql("c");
    format!(
        "SELECT c.hash, l.representative_path_id, rp.file_name, l.resolved_utc_ms, \
            l.live_copy_count, c.width, c.height, \
            (l.kind IN ('image', 'video') AND {preview_available}), \
            (SELECT m.group_id FROM similar_group_members m \
             WHERE m.content_hash = c.hash LIMIT 1), \
            (SELECT COUNT(*) FROM similar_group_members members \
             JOIN review_contents member_items \
               ON member_items.content_hash = members.content_hash \
             WHERE member_items.live_copy_count > 0 \
               AND members.group_id = (SELECT m.group_id FROM similar_group_members m \
                                       WHERE m.content_hash = c.hash LIMIT 1)), \
            c.sharpness, c.byte_size, \
            EXISTS (SELECT 1 FROM paths comp JOIN paths pri ON comp.companion_of = pri.id \
                    WHERE pri.content_hash = c.hash AND comp.missing = 0 \
                      AND pri.missing = 0), \
            c.duration_ms, c.kind, c.derived_at_utc, \
            c.derived_version, c.strip_frames, {face_state}, {face_score}, \
            {transcript_state}, \
            EXISTS (
              SELECT 1 FROM similarity_dirty_buckets dirty
              WHERE dirty.bucket = COALESCE(
                strftime('%Y-%m', l.resolved_utc_ms / 1000.0, 'unixepoch'),
                'undated'
              )
            ), \
            c.derive_outcome \
     FROM review_contents l \
     JOIN contents c ON c.hash = l.content_hash \
     JOIN paths rp ON rp.id = l.representative_path_id "
    )
}

fn section_item_from_row(
    row: &rusqlite::Row<'_>,
    projection: ItemProjectionContext,
) -> rusqlite::Result<SectionItem> {
    let kind: String = row.get(14)?;
    let derived_at: Option<String> = row.get(15)?;
    let derive_outcome: Option<String> = row.get(22)?;
    let face_state: Option<String> = row.get(18)?;
    let transcript_state: Option<String> = row.get(20)?;
    Ok(SectionItem {
        hash: Some(row.get(0)?),
        path_id: row.get(1)?,
        file_name: row.get(2)?,
        resolved_utc_ms: row.get(3)?,
        copy_count: row.get::<_, i64>(4)?.max(0) as u64,
        width: row.get(5)?,
        height: row.get(6)?,
        has_thumb: row.get(7)?,
        similar_group_id: row.get(8)?,
        similar_count: row.get::<_, i64>(9)?.max(0) as u64,
        sharpness: row.get(10)?,
        face_score: row.get(19)?,
        byte_size: row.get(11)?,
        has_companions: row.get(12)?,
        duration_ms: row.get(13)?,
        dir_paths: Vec::new(),
        derived_work: crate::derived_state::item_work_states(
            crate::derived_state::ItemWorkFacts {
                kind: &kind,
                derived_at: derived_at.as_deref(),
                derive_outcome: derive_outcome.as_deref(),
                derived_version: row.get(16)?,
                strip_frames: row.get(17)?,
                duration_ms: row.get(13)?,
                similar_group_id: row.get(8)?,
                face_state: face_state.as_deref(),
                face_score: row.get(19)?,
                transcript_state: transcript_state.as_deref(),
            },
            projection.capabilities,
            row.get(21)?,
        ),
    })
}

fn section_dir_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String)> {
    Ok((row.get(0)?, row.get(1)?))
}

/// One comparison-view member: enough to render a preview tile, ORDER the
/// group best-first, and tell two versions of the same picture apart.
///
/// `byte_size` and the dimensions carry that last job. A group is very often
/// one shot at three qualities — the camera original, an export, and a
/// downscaled copy for the web — and at slot size they are the same image. The
/// keep-one-delete-the-rest flow is undecidable without the numbers.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GroupMember {
    pub hash: String,
    pub file_name: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub byte_size: Option<i64>,
    pub sharpness: Option<f64>,
    pub face_score: Option<f64>,
    pub copy_count: u64,
    pub has_thumb: bool,
}

/// Every member of the similar group containing `hash`, best-first: face
/// score, then sharpness (both advisory machine guesses); empty when the item
/// is ungrouped. COALESCE makes NULL and scored-faceless order identically,
/// so a group with no faces — or no face models — orders exactly by
/// sharpness, as before the models existed.
pub fn similar_group_of(
    conn: &Connection,
    hash: &str,
    use_face_score: bool,
) -> Result<Vec<GroupMember>, String> {
    let group_id: Option<i64> = conn
        .query_row(
            "SELECT group_id FROM similar_group_members WHERE content_hash = ?1",
            [hash],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(group_id) = group_id else {
        return Ok(Vec::new());
    };

    let preview_available = crate::derived_state::preview_available_predicate("c");
    let face_score = crate::derived_state::face_score_sql("c");
    let mut stmt = conn
        .prepare(&format!(
            "SELECT c.hash, representative.file_name, \
             c.width, c.height, c.byte_size, c.sharpness, {face_score}, \
             logical.live_copy_count, \
             {preview_available} \
             FROM similar_group_members m \
             JOIN contents c ON c.hash = m.content_hash \
             JOIN review_contents logical ON logical.content_hash = c.hash \
             JOIN paths representative ON representative.id = logical.representative_path_id \
             WHERE m.group_id = ?1 \
             ORDER BY CASE WHEN ?2 THEN COALESCE({face_score}, 0) ELSE 0 END DESC, \
                      c.sharpness DESC NULLS LAST, \
                      representative.abs_path COLLATE onecopy_nocase, \
                      representative.abs_path, c.hash"
        ))
        .map_err(|e| e.to_string())?;
    let members: Vec<GroupMember> = stmt
        .query_map(rusqlite::params![group_id, use_face_score], |r| {
            Ok(GroupMember {
                hash: r.get(0)?,
                file_name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                width: r.get(2)?,
                height: r.get(3)?,
                byte_size: r.get(4)?,
                sharpness: r.get(5)?,
                face_score: r.get(6)?,
                copy_count: r.get::<_, i64>(7)?.max(0) as u64,
                has_thumb: r.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|m| m.copy_count > 0)
        .collect();
    Ok(members)
}

/// The subset of a frozen Comparison membership that still has at least one
/// indexed live copy. Newly grouped hashes are deliberately not returned.
pub fn live_content_hashes(
    conn: &Connection,
    hashes: &[String],
) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT EXISTS(SELECT 1 FROM review_contents WHERE content_hash = ?1)",
        )
        .map_err(|error| error.to_string())?;
    let mut live = Vec::with_capacity(hashes.len());
    for hash in hashes {
        let exists = statement
            .query_row([hash], |row| row.get::<_, bool>(0))
            .map_err(|error| error.to_string())?;
        if exists {
            live.push(hash.clone());
        }
    }
    Ok(live)
}

/// The metadata pane's view of one logical item: content facts plus every
/// copy path (the copy list doubles as the user's backup health check) and any
/// companions riding along.
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetail {
    pub file_name: String,
    pub kind: String,
    pub byte_size: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
    pub date_state: String,
    pub resolved_utc_ms: Option<i64>,
    pub resolved_source: Option<String>,
    pub date_only: bool,
    pub copy_paths: Vec<String>,
    pub companion_paths: Vec<String>,
    pub strip_frames: Option<i64>,
}

pub fn item_detail(
    conn: &Connection,
    hash: Option<&str>,
    path_id: Option<i64>,
) -> Result<ItemDetail, String> {
    let copies: Vec<(
        i64,
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<String>,
        i64,
    )> = match (hash, path_id) {
        (Some(hash), _) => {
            let mut stmt = conn
                .prepare(
                    "SELECT p.id, p.abs_path, p.file_name, p.kind, p.size, \
                         p.resolved_utc_ms, p.resolved_source, p.date_only \
                         FROM paths p WHERE p.content_hash = ?1 AND p.missing = 0 AND p.companion_of IS NULL \
                         ORDER BY p.review_visible DESC, p.resolved_utc_ms IS NULL, p.resolved_utc_ms, \
                                  p.abs_path COLLATE onecopy_nocase, p.abs_path",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([hash], row_to_copy)
                    .map_err(|e| e.to_string())?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| e.to_string())?;
                rows
            }
            (None, Some(id)) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT p.id, p.abs_path, p.file_name, p.kind, p.size, \
                         p.resolved_utc_ms, p.resolved_source, p.date_only \
                         FROM paths p WHERE p.id = ?1 AND p.missing = 0 AND p.review_visible = 1",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([id], row_to_copy)
                    .map_err(|e| e.to_string())?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| e.to_string())?;
                rows
            }
            (None, None) => return Err("item_detail needs a hash or a pathId".to_string()),
        };

    let Some(first) = copies.first() else {
        return Err("item not found".to_string());
    };

    let (date_state, resolved_utc_ms) = match hash {
        Some(hash) => conn
            .query_row(
                "SELECT date_state, resolved_utc_ms FROM review_contents \
                 WHERE content_hash = ?1",
                [hash],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .map_err(|error| error.to_string())?,
        None => match (&first.6, first.5) {
            (None, _) => ("pending".to_string(), None),
            (Some(_), Some(value)) => ("dated".to_string(), Some(value)),
            (Some(_), None) => ("undated".to_string(), None),
        },
    };

    let (width, height, duration_ms, byte_size, strip_frames) = match hash {
        Some(hash) => conn
            .query_row(
                "SELECT width, height, duration_ms, byte_size, strip_frames \
                 FROM contents WHERE hash = ?1",
                [hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or((None, None, None, first.4, None)),
        None => (None, None, None, first.4, None),
    };

    let id_list = copies
        .iter()
        .map(|c| c.0.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut stmt = conn
        .prepare(&format!(
            "SELECT abs_path FROM paths WHERE companion_of IN ({id_list}) AND missing = 0 \
             ORDER BY abs_path"
        ))
        .map_err(|e| e.to_string())?;
    let companion_paths: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|path| crate::winpath::for_display(&path).into_owned())
        .collect();
    drop(stmt);
    let date_copy = copies.iter().filter(|copy| copy.5 == resolved_utc_ms)
        .min_by(|left, right| left.1.to_lowercase().cmp(&right.1.to_lowercase()).then_with(|| left.1.cmp(&right.1)));
    let resolved_source = if date_state == "pending" {
        None
    } else {
        date_copy.and_then(|copy| copy.6.clone())
    };

    Ok(ItemDetail {
        file_name: first.2.clone(),
        kind: first.3.clone(),
        byte_size,
        width,
        height,
        duration_ms,
        date_state,
        resolved_utc_ms,
        resolved_source,
        date_only: resolved_utc_ms.is_some() && date_copy.is_some_and(|copy| copy.7 != 0),
        // Keep the verbatim spelling in SQLite for filesystem work, but never
        // make the Windows implementation detail part of a user-facing path.
        copy_paths: copies
            .iter()
            .map(|c| crate::winpath::for_display(&c.1).into_owned())
            .collect(),
        companion_paths,
        strip_frames,
    })
}

/// The directories that contributed files to one (kind, month) section — the
/// scoped-rescan unit: re-stat exactly these, never the whole roots.
fn section_dirs_sql(has_bounds: bool) -> String {
    let time_clause = if has_bounds {
        "AND l.resolved_utc_ms >= ?2 AND l.resolved_utc_ms < ?3"
    } else {
        "AND l.resolved_utc_ms IS NULL"
    };
    format!(
        "SELECT DISTINCT p.dir_path
         FROM review_contents l
         JOIN paths p ON p.content_hash = l.content_hash
         WHERE l.kind = ?1 {time_clause}
           AND p.missing = 0 AND p.companion_of IS NULL"
    )
}

fn unhashed_other_section_dirs_sql(has_bounds: bool) -> String {
    let time_clause = if has_bounds {
        "AND resolved_utc_ms >= ?1 AND resolved_utc_ms < ?2"
    } else {
        "AND resolved_utc_ms IS NULL"
    };
    format!(
        "SELECT DISTINCT dir_path
         FROM paths INDEXED BY idx_paths_unhashed_other_section
         WHERE missing = 0 AND review_visible = 1 AND companion_of IS NULL AND content_hash IS NULL
           AND kind NOT IN ('image', 'video') {time_clause}"
    )
}

pub fn section_dirs(
    conn: &Connection,
    kind: SectionKind,
    month: &str,
    display_tz: Tz,
) -> Result<Vec<String>, String> {
    let kind = kind.as_str();
    let bounds = month_bounds(month, display_tz)?;
    let sql = section_dirs_sql(bounds.is_some());
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let mut dirs: Vec<String> = match bounds {
        Some((start, end)) => statement
            .query_map(rusqlite::params![kind, start, end], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?,
        None => statement
            .query_map([kind], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?,
    };
    drop(statement);

    if kind == "other" {
        let sql = unhashed_other_section_dirs_sql(bounds.is_some());
        let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
        let other_dirs: Vec<String> = match bounds {
            Some((start, end)) => statement
                .query_map(rusqlite::params![start, end], |row| row.get(0))
                .map_err(|error| error.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| error.to_string())?,
            None => statement
                .query_map([], |row| row.get(0))
                .map_err(|error| error.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| error.to_string())?,
        };
        dirs.extend(other_dirs);
    }
    dirs.sort();
    dirs.dedup();
    Ok(dirs)
}

/// One issues row for the issues modal. `path` is None when the row has no
/// file anchor (stored as '' for the (kind, path) identity).
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IssueRow {
    pub id: i64,
    pub path: Option<String>,
    pub kind: String,
    pub message: Option<String>,
    /// A catalogue key the frontend renders in the current interface
    /// language; absent for a row recorded before this descriptor existed,
    /// or one whose text has no translatable sentence to key (R5.5 D-L12).
    pub message_key: Option<String>,
    /// The key's interpolation values, as a JSON object; absent when the key
    /// takes none.
    pub message_values: Option<serde_json::Value>,
    pub first_seen_utc: String,
    pub last_seen_utc: String,
    pub occurrence_count: u64,
}

const ISSUES_PAGE_SQL: &str =
    "SELECT id, path, kind, message, message_key, message_values, first_seen_utc, last_seen_utc, occurrence_count FROM active_issues
     ORDER BY first_seen_utc ASC, id ASC LIMIT ?1";

const ISSUES_PAGE_AFTER_SQL: &str =
    "SELECT id, path, kind, message, message_key, message_values, first_seen_utc, last_seen_utc, occurrence_count FROM active_issues
     WHERE (first_seen_utc, id) > (?2, ?3)
     ORDER BY first_seen_utc ASC, id ASC LIMIT ?1";

/// The largest page `get_issues` serves regardless of the client-supplied
/// limit, matching `activity::MAX_PAGE_SIZE`'s bound on the analogous
/// Activity page (C-L5: an unclamped client limit could otherwise pull every
/// Issue row and its JSON on each poll).
const MAX_ISSUES_PAGE_SIZE: u32 = 500;

/// Where the previous page ended, so the next one picks up right after it —
/// keyset paging on the same order the page is sorted by (oldest first), so a
/// row past the 500th is reachable and dismissing earlier rows cannot skip or
/// repeat a later one the way an OFFSET page would (R4.4 finding A).
pub struct IssuesCursor {
    pub first_seen_utc: String,
    pub id: i64,
}

/// OLDEST first (the developer's call — the longest-standing condition leads),
/// capped; the count comes with it for the status-bar element. `after` is the
/// previous page's last row, absent for the first page.
pub fn issues(
    conn: &Connection,
    limit: u32,
    after: Option<&IssuesCursor>,
) -> Result<(u64, Vec<IssueRow>), String> {
    let limit = limit.clamp(1, MAX_ISSUES_PAGE_SIZE);
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM active_issues", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let row_mapper = |r: &rusqlite::Row| -> rusqlite::Result<IssueRow> {
        let path: String = r.get(1)?;
        // Issue rows are written straight from `abs_path`, and on Windows
        // EVERY indexed path is stored verbatim (`for_fs` is unconditional
        // there, not length-gated) — so without this the issues list shows
        // `\\?\C:\…` for every file, not just deep ones. The stored
        // spelling stays verbatim: issue identity is (kind, path), and
        // `clear_issues` matches on what the pipeline wrote.
        let message_values: Option<String> = r.get(5)?;
        Ok(IssueRow {
            id: r.get(0)?,
            path: if path.is_empty() {
                None
            } else {
                Some(crate::winpath::for_display(&path).into_owned())
            },
            kind: r.get(2)?,
            message: r.get(3)?,
            message_key: r.get(4)?,
            message_values: message_values.and_then(|json| serde_json::from_str(&json).ok()),
            first_seen_utc: r.get(6)?,
            last_seen_utc: r.get(7)?,
            occurrence_count: r.get::<_, i64>(8)?.max(1) as u64,
        })
    };
    let rows: Vec<IssueRow> = match after {
        None => {
            let mut stmt = conn.prepare(ISSUES_PAGE_SQL).map_err(|e| e.to_string())?;
            let mapped = stmt
                .query_map([limit], row_mapper)
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            mapped
        }
        Some(cursor) => {
            let mut stmt = conn
                .prepare(ISSUES_PAGE_AFTER_SQL)
                .map_err(|e| e.to_string())?;
            let mapped = stmt
                .query_map(
                    rusqlite::params![limit, cursor.first_seen_utc, cursor.id],
                    row_mapper,
                )
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            mapped
        }
    };
    Ok((total.max(0) as u64, rows))
}

#[allow(clippy::type_complexity)]
fn row_to_copy(
    r: &rusqlite::Row,
) -> rusqlite::Result<(
    i64,
    String,
    String,
    String,
    Option<i64>,
    Option<i64>,
    Option<String>,
    i64,
)> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
    ))
}

pub(crate) fn month_bounds(month: &str, display_tz: Tz) -> Result<Option<(i64, i64)>, String> {
    if month == "undated" {
        return Ok(None);
    }
    let (year, mon) = month
        .split_once('-')
        .and_then(|(year, mon)| Some((year.parse::<i32>().ok()?, mon.parse::<u32>().ok()?)))
        .filter(|(_, mon)| (1..=12).contains(mon))
        .ok_or_else(|| format!("bad month key: {month}"))?;
    let (next_year, next_mon) = if mon == 12 {
        (year.checked_add(1).ok_or_else(|| format!("bad month key: {month}"))?, 1)
    } else {
        (year, mon + 1)
    };
    // A local month need not start at an existing midnight. Use the actual
    // transition boundary rather than rejecting every query for that month
    // (and its predecessor), or inventing a UTC interpretation of local time.
    let boundary = |year, month| {
        let local = chrono::NaiveDate::from_ymd_opt(year, month, 1)?.and_hms_opt(0, 0, 0)?;
        display_tz.from_local_datetime(&local).earliest()
            .or_else(|| chrono_tz::GapInfo::new(&local, &display_tz)?.end)
            .map(|instant| instant.timestamp_millis())
    };
    let start = boundary(year, mon).ok_or_else(|| format!("bad month start: {month}"))?;
    let end = boundary(next_year, next_mon).ok_or_else(|| format!("bad month end: {month}"))?;
    Ok(Some((start, end)))
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private section and
// Issues SQL builders; promoting them would widen the crate's API only for
// this test.
#[path = "../tests/unit/queries.rs"]
mod tests;

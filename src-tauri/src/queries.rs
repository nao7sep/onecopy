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
use std::sync::Mutex;

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

const LOGICAL_MONTH_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM logical_contents INDEXED BY idx_logical_contents_section
     WHERE kind = ?1 AND resolved_utc_ms >= ?2 AND resolved_utc_ms < ?3";
const UNHASHED_OTHER_MONTH_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM paths INDEXED BY idx_paths_unhashed_other_section
     WHERE missing = 0 AND companion_of IS NULL AND content_hash IS NULL
       AND kind NOT IN ('image', 'video')
       AND resolved_utc_ms >= ?1 AND resolved_utc_ms < ?2";
const LOGICAL_UNDATED_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM logical_contents INDEXED BY idx_logical_contents_section
     WHERE kind = ?1 AND resolved_utc_ms IS NULL";
const UNHASHED_OTHER_UNDATED_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM paths INDEXED BY idx_paths_unhashed_other_section
     WHERE missing = 0 AND companion_of IS NULL AND content_hash IS NULL
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
         WHERE kind = ?1 AND resolved_utc_ms IS NOT NULL
         ORDER BY resolved_utc_ms {direction} LIMIT 1"
    )
}

fn unhashed_other_edge_sql(newest: bool) -> String {
    let direction = if newest { "DESC" } else { "ASC" };
    format!(
        "SELECT resolved_utc_ms
         FROM paths INDEXED BY idx_paths_unhashed_other_section
         WHERE missing = 0 AND companion_of IS NULL AND content_hash IS NULL
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
pub fn cached_section_counts(db_file: &Path, display_tz: Tz) -> Result<SectionCounts, String> {
    let mut cache = SECTION_COUNTS_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let needs_connection = cache
        .as_ref()
        .is_none_or(|existing| existing.db_file != db_file);
    if needs_connection {
        *cache = Some(SectionCountsCache::open(db_file)?);
    }
    let cache = cache
        .as_mut()
        .ok_or_else(|| "section count cache could not be initialized".to_string())?;
    cache.load(display_tz).map(|(counts, _)| counts)
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

#[derive(Clone, Copy)]
pub struct ItemProjectionContext {
    pub capabilities: crate::derived_state::WorkCapabilities,
    pub similarity_dirty: bool,
}

#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum SectionSortOrder {
    Time,
    Name,
    Size,
    Resolution,
    Ext,
}

#[derive(Deserialize, Debug, Clone, Copy)]
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
    kind: &str,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    start: u64,
    limit: u32,
    projection: ItemProjectionContext,
) -> Result<SectionWindow, String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .map_err(|error| error.to_string())?;
    let window = section_window_snapshot(
        &transaction,
        kind,
        month,
        display_tz,
        sort,
        start,
        limit,
        projection,
    )?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(window)
}

#[allow(clippy::too_many_arguments)]
fn section_window_snapshot(
    conn: &Connection,
    kind: &str,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    start: u64,
    limit: u32,
    projection: ItemProjectionContext,
) -> Result<SectionWindow, String> {
    if !matches!(kind, "image" | "video" | "other") {
        return Err(format!("bad section kind: {kind}"));
    }
    if !(1..=MAX_SECTION_WINDOW_ITEMS).contains(&limit) {
        return Err(format!(
            "section window limit must be between 1 and {MAX_SECTION_WINDOW_ITEMS}"
        ));
    }
    let bounds = month_bounds(month, display_tz)?;
    let candidates = section_candidates_sql(kind == "other", bounds.is_some());
    let base_params = section_candidate_params(kind, bounds);
    let total_sql = format!("WITH candidates AS ({candidates}) SELECT COUNT(*) FROM candidates");
    let total = conn
        .query_row(
            &total_sql,
            rusqlite::params_from_iter(base_params.iter()),
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| error.to_string())?
        .max(0) as u64;
    let start = start.min(total);
    if start == total {
        return Ok(SectionWindow {
            total,
            start,
            items: Vec::new(),
        });
    }

    let mut page_params = base_params;
    page_params.push((limit as i64).into());
    page_params.push((start.min(i64::MAX as u64) as i64).into());
    let page_sql = format!(
        "WITH candidates AS ({candidates}) \
         SELECT hash, path_id FROM candidates ORDER BY {} LIMIT ?{} OFFSET ?{}",
        section_order_sql(sort),
        page_params.len() - 1,
        page_params.len(),
    );
    let mut statement = conn.prepare(&page_sql).map_err(|error| error.to_string())?;
    let identities = statement
        .query_map(rusqlite::params_from_iter(page_params.iter()), |row| {
            Ok(SectionIdentity {
                hash: row.get(0)?,
                path_id: row.get(1)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    drop(statement);

    Ok(SectionWindow {
        total,
        start,
        items: section_items_by_identity(conn, &identities, projection)?,
    })
}

const RECOVERY_NEIGHBOR_LIMIT: u64 = 64;

/// Reconciles selection and the work-position anchor against one ordered
/// section snapshot, then returns only the display window around the chosen
/// anchor. The scan retains matches for explicitly supplied identities; it
/// never materializes the section itself.
#[allow(clippy::too_many_arguments)]
pub fn reconcile_section(
    conn: &Connection,
    kind: &str,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    selected: &[SectionIdentity],
    anchor: Option<&SectionIdentity>,
    range_origin: Option<&SectionIdentity>,
    range_base: &[SectionIdentity],
    recovery: Option<&SectionRecoveryContext>,
    select_first: bool,
    limit: u32,
    projection: ItemProjectionContext,
) -> Result<SectionReconciliation, String> {
    if !matches!(kind, "image" | "video" | "other") {
        return Err(format!("bad section kind: {kind}"));
    }
    if !(1..=MAX_SECTION_WINDOW_ITEMS).contains(&limit) {
        return Err(format!(
            "section window limit must be between 1 and {MAX_SECTION_WINDOW_ITEMS}"
        ));
    }
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .map_err(|error| error.to_string())?;
    let bounds = month_bounds(month, display_tz)?;
    let candidates = section_candidates_sql(kind == "other", bounds.is_some());
    let params = section_candidate_params(kind, bounds);
    let sql = format!(
        "WITH candidates AS ({candidates}) \
         SELECT hash, path_id FROM candidates ORDER BY {}",
        section_order_sql(sort)
    );

    let wanted: HashSet<String> = selected
        .iter()
        .chain(anchor)
        .chain(range_origin)
        .chain(range_base)
        .chain(recovery.into_iter().flat_map(|context| {
            context.before.iter().chain(context.after.iter())
        }))
        .map(SectionIdentity::key)
        .collect();
    let mut matches = HashMap::<String, PositionedSectionIdentity>::with_capacity(wanted.len());
    let mut statement = transaction
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|error| error.to_string())?;
    let mut total = 0_u64;
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let identity = SectionIdentity {
            hash: row.get(0).map_err(|error| error.to_string())?,
            path_id: row.get(1).map_err(|error| error.to_string())?,
        };
        let key = identity.key();
        if wanted.contains(&key) {
            matches.insert(
                key,
                PositionedSectionIdentity::from_identity(identity, total),
            );
        }
        total += 1;
    }
    drop(rows);
    drop(statement);

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
        select_first,
        total,
    );
    let anchor = match chosen {
        Some(AnchorChoice::Known(member)) => Some(member),
        Some(AnchorChoice::Index(index)) => section_identity_at(
            &transaction,
            kind,
            bounds,
            sort,
            index,
        )?
        .map(|identity| PositionedSectionIdentity::from_identity(identity, index)),
        None => None,
    };

    let anchor_index = anchor.as_ref().map_or(0, |member| member.index);
    let max_start = total.saturating_sub(u64::from(limit));
    let start = anchor_index
        .saturating_sub(u64::from(limit) / 2)
        .min(max_start);
    let window = section_window_snapshot(
        &transaction,
        kind,
        month,
        display_tz,
        sort,
        start,
        limit,
        projection,
    )?;
    let context = anchor
        .as_ref()
        .map(|member| section_recovery_context(&transaction, kind, bounds, sort, member.index))
        .transpose()?;
    transaction.commit().map_err(|error| error.to_string())?;

    Ok(SectionReconciliation {
        anchor,
        selected: live_selected,
        range_origin: live_range_origin,
        range_base: live_range_base,
        context,
        window,
    })
}

fn matched_identities(
    requested: &[SectionIdentity],
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
        if let Some(member) = live_selected
            .iter()
            .find(|member| member.index >= context.index)
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

fn section_identity_at(
    conn: &Connection,
    kind: &str,
    bounds: Option<(i64, i64)>,
    sort: SectionSort,
    index: u64,
) -> Result<Option<SectionIdentity>, String> {
    section_identity_range(conn, kind, bounds, sort, index, index.saturating_add(1))
        .map(|mut identities| identities.pop())
}

fn section_recovery_context(
    conn: &Connection,
    kind: &str,
    bounds: Option<(i64, i64)>,
    sort: SectionSort,
    index: u64,
) -> Result<SectionRecoveryContextOutput, String> {
    let start = index.saturating_sub(RECOVERY_NEIGHBOR_LIMIT);
    let end = index.saturating_add(RECOVERY_NEIGHBOR_LIMIT + 1);
    let identities = section_identity_range(conn, kind, bounds, sort, start, end)?;
    let anchor_offset = (index - start) as usize;
    Ok(SectionRecoveryContextOutput {
        index,
        before: identities[..anchor_offset].iter().rev().cloned().collect(),
        after: identities.get(anchor_offset + 1..).unwrap_or_default().to_vec(),
    })
}

fn section_identity_range(
    conn: &Connection,
    kind: &str,
    bounds: Option<(i64, i64)>,
    sort: SectionSort,
    start: u64,
    end: u64,
) -> Result<Vec<SectionIdentity>, String> {
    if end <= start {
        return Ok(Vec::new());
    }
    let candidates = section_candidates_sql(kind == "other", bounds.is_some());
    let mut params = section_candidate_params(kind, bounds);
    let limit = end.saturating_sub(start).min(i64::MAX as u64) as i64;
    params.push(limit.into());
    params.push((start.min(i64::MAX as u64) as i64).into());
    let sql = format!(
        "WITH candidates AS ({candidates}) \
         SELECT hash, path_id FROM candidates ORDER BY {} LIMIT ?{} OFFSET ?{}",
        section_order_sql(sort),
        params.len() - 1,
        params.len(),
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
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
    Ok(identities)
}

/// Returns the ordered identities for one deliberate Main range-selection.
/// The result may be large because its size is the user's explicit selection,
/// while ordinary browsing and rendering remain capped.
pub fn section_range(
    conn: &Connection,
    kind: &str,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    start: u64,
    end: u64,
) -> Result<Vec<PositionedSectionIdentity>, String> {
    if !matches!(kind, "image" | "video" | "other") {
        return Err(format!("bad section kind: {kind}"));
    }
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .map_err(|error| error.to_string())?;
    let bounds = month_bounds(month, display_tz)?;
    let members = section_identity_range(&transaction, kind, bounds, sort, start, end)?
        .into_iter()
        .enumerate()
        .map(|(offset, identity)| {
            PositionedSectionIdentity::from_identity(identity, start + offset as u64)
        })
        .collect();
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(members)
}

/// Captures the bounded Main recovery neighborhood after the last member of
/// a Comparison family in the current order. Comparison keeps this before it
/// changes files, so Main can return past the original family afterward.
pub fn section_family_context(
    conn: &Connection,
    kind: &str,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    member_hashes: &[String],
) -> Result<Option<SectionRecoveryContextOutput>, String> {
    if !matches!(kind, "image" | "video" | "other") {
        return Err(format!("bad section kind: {kind}"));
    }
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .map_err(|error| error.to_string())?;
    let bounds = month_bounds(month, display_tz)?;
    let family: HashSet<&str> = member_hashes.iter().map(String::as_str).collect();
    let candidates = section_candidates_sql(kind == "other", bounds.is_some());
    let params = section_candidate_params(kind, bounds);
    let sql = format!(
        "WITH candidates AS ({candidates}) \
         SELECT hash FROM candidates ORDER BY {}",
        section_order_sql(sort)
    );
    let mut statement = transaction.prepare(&sql).map_err(|error| error.to_string())?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|error| error.to_string())?;
    let mut index = 0_u64;
    let mut last_member = None;
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let hash: Option<String> = row.get(0).map_err(|error| error.to_string())?;
        if hash.as_deref().is_some_and(|value| family.contains(value)) {
            last_member = Some(index);
        }
        index += 1;
    }
    drop(rows);
    drop(statement);
    let context = last_member
        .map(|member_index| {
            section_recovery_context(&transaction, kind, bounds, sort, member_index)
        })
        .transpose()?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(context)
}

/// Streams one complete ordered identity sequence without collecting it.
/// The caller may persist it in a disposable store while this read
/// transaction keeps membership and order on one index snapshot.
pub fn visit_section_identities(
    conn: &Connection,
    kind: &str,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    mut visit: impl FnMut(u64, &SectionIdentity) -> Result<(), String>,
) -> Result<u64, String> {
    if !matches!(kind, "image" | "video" | "other") {
        return Err(format!("bad section kind: {kind}"));
    }
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
            .map_err(|error| error.to_string())?;
    let bounds = month_bounds(month, display_tz)?;
    let candidates = section_candidates_sql(kind == "other", bounds.is_some());
    let params = section_candidate_params(kind, bounds);
    let sql = format!(
        "WITH candidates AS ({candidates}) \
         SELECT hash, path_id FROM candidates ORDER BY {}",
        section_order_sql(sort)
    );
    let mut statement = transaction.prepare(&sql).map_err(|error| error.to_string())?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|error| error.to_string())?;
    let mut count = 0_u64;
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        let identity = SectionIdentity {
            hash: row.get(0).map_err(|error| error.to_string())?,
            path_id: row.get(1).map_err(|error| error.to_string())?,
        };
        visit(count, &identity)?;
        count += 1;
    }
    drop(rows);
    drop(statement);
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(count)
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
         FROM logical_contents l \
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
             WHERE p.missing = 0 AND p.companion_of IS NULL \
               AND p.content_hash IS NULL AND p.kind NOT IN ('image', 'video') \
               AND {other_time}"
        ));
    }
    sql
}

fn section_order_sql(sort: SectionSort) -> String {
    let direction = if sort.desc { "DESC" } else { "ASC" };
    let null_direction = if sort.desc { "DESC" } else { "ASC" };
    let time = "resolved_utc_ms IS NULL ASC, resolved_utc_ms ASC";
    let name = "file_name COLLATE onecopy_nocase ASC";
    let primary = match sort.order {
        SectionSortOrder::Time => {
            format!("resolved_utc_ms IS NULL {null_direction}, resolved_utc_ms {direction}")
        }
        SectionSortOrder::Name => format!("file_name COLLATE onecopy_nocase {direction}"),
        SectionSortOrder::Size => format!("COALESCE(byte_size, -1) {direction}"),
        SectionSortOrder::Resolution => {
            format!("(COALESCE(width, 0) * COALESCE(height, 0)) {direction}")
        }
        SectionSortOrder::Ext => format!("COALESCE(extension, '') {direction}"),
    };
    let chain = match sort.order {
        SectionSortOrder::Time => name.to_string(),
        SectionSortOrder::Name => time.to_string(),
        SectionSortOrder::Size | SectionSortOrder::Resolution => format!("{time}, {name}"),
        SectionSortOrder::Ext => name.to_string(),
    };
    format!("{primary}, {chain}, path_id ASC")
}

fn identity_key(hash: Option<&str>, path_id: i64) -> String {
    hash.map_or_else(|| format!("path-{path_id}"), str::to_owned)
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
             WHERE id IN ({placeholders}) AND missing = 0 AND companion_of IS NULL \
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
                derived_version: 0,
                strip_frames: None,
                duration_ms: None,
                similar_group_id: None,
                face_state: None,
                face_score: None,
                transcript_state: None,
            },
            projection.capabilities,
            projection.similarity_dirty,
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
         WHERE id = ?1 AND missing = 0 AND companion_of IS NULL \
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
             JOIN logical_contents l ON l.content_hash = m.content_hash \
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
    format!(
        "SELECT c.hash, l.representative_path_id, rp.file_name, l.resolved_utc_ms, \
            l.live_copy_count, c.width, c.height, \
            (l.kind IN ('image', 'video') AND {preview_available}), \
            (SELECT m.group_id FROM similar_group_members m \
             WHERE m.content_hash = c.hash LIMIT 1), \
            (SELECT COUNT(*) FROM similar_group_members members \
             JOIN logical_contents member_items \
               ON member_items.content_hash = members.content_hash \
             WHERE member_items.live_copy_count > 0 \
               AND members.group_id = (SELECT m.group_id FROM similar_group_members m \
                                       WHERE m.content_hash = c.hash LIMIT 1)), \
            c.sharpness, c.byte_size, \
            EXISTS (SELECT 1 FROM paths comp JOIN paths pri ON comp.companion_of = pri.id \
                    WHERE pri.content_hash = c.hash AND comp.missing = 0 \
                      AND pri.missing = 0), \
            c.duration_ms, c.kind, c.derived_at_utc, \
            c.derived_version, c.strip_frames, r.face_state, c.face_score, \
            r.transcript_state \
     FROM logical_contents l \
     JOIN contents c ON c.hash = l.content_hash \
     JOIN paths rp ON rp.id = l.representative_path_id \
     LEFT JOIN analysis_receipts r ON r.content_hash = c.hash "
    )
}

fn section_item_from_row(
    row: &rusqlite::Row<'_>,
    projection: ItemProjectionContext,
) -> rusqlite::Result<SectionItem> {
    let kind: String = row.get(14)?;
    let derived_at: Option<String> = row.get(15)?;
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
                derived_version: row.get(16)?,
                strip_frames: row.get(17)?,
                duration_ms: row.get(13)?,
                similar_group_id: row.get(8)?,
                face_state: face_state.as_deref(),
                face_score: row.get(19)?,
                transcript_state: transcript_state.as_deref(),
            },
            projection.capabilities,
            projection.similarity_dirty,
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
    let mut stmt = conn
        .prepare(&format!(
            "SELECT c.hash, representative.file_name, \
             c.width, c.height, c.byte_size, c.sharpness, c.face_score, \
             (SELECT COUNT(*) FROM paths p WHERE p.content_hash = c.hash AND p.missing = 0), \
             {preview_available} \
             FROM similar_group_members m \
             JOIN contents c ON c.hash = m.content_hash \
             JOIN logical_contents logical ON logical.content_hash = c.hash \
             JOIN paths representative ON representative.id = logical.representative_path_id \
             WHERE m.group_id = ?1 \
             ORDER BY CASE WHEN ?2 THEN COALESCE(c.face_score, 0) ELSE 0 END DESC, \
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
            "SELECT EXISTS(SELECT 1 FROM paths WHERE content_hash = ?1 AND missing = 0)",
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
                         FROM paths p WHERE p.content_hash = ?1 AND p.missing = 0 \
                         ORDER BY p.resolved_utc_ms IS NULL, p.resolved_utc_ms, \
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
                         FROM paths p WHERE p.id = ?1 AND p.missing = 0",
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
                "SELECT date_state, resolved_utc_ms FROM logical_contents \
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
    let resolved_source = if date_state == "pending" {
        None
    } else {
        first.6.clone()
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
        date_only: resolved_utc_ms.is_some() && first.7 != 0,
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
         FROM logical_contents l
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
         WHERE missing = 0 AND companion_of IS NULL AND content_hash IS NULL
           AND kind NOT IN ('image', 'video') {time_clause}"
    )
}

pub fn section_dirs(
    conn: &Connection,
    kind: &str,
    month: &str,
    display_tz: Tz,
) -> Result<Vec<String>, String> {
    if !matches!(kind, "image" | "video" | "other") {
        return Err(format!("bad section kind: {kind}"));
    }
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
    pub first_seen_utc: String,
    pub last_seen_utc: String,
    pub occurrence_count: u64,
    pub recovery: Option<crate::issue_recovery::IssueRecovery>,
}

const ISSUES_PAGE_SQL: &str =
    "SELECT id, path, kind, message, first_seen_utc, last_seen_utc, occurrence_count FROM issues
     ORDER BY first_seen_utc ASC, id ASC LIMIT ?1";

/// OLDEST first (the developer's call — the longest-standing condition leads),
/// capped; the count comes with it for the status-bar element.
pub fn issues(conn: &Connection, limit: u32) -> Result<(u64, Vec<IssueRow>), String> {
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(ISSUES_PAGE_SQL)
        .map_err(|e| e.to_string())?;
    let mut rows: Vec<IssueRow> = stmt
        .query_map([limit], |r| {
            let path: String = r.get(1)?;
            // Issue rows are written straight from `abs_path`, and on Windows
            // EVERY indexed path is stored verbatim (`for_fs` is unconditional
            // there, not length-gated) — so without this the issues list shows
            // `\\?\C:\…` for every file, not just deep ones. The stored
            // spelling stays verbatim: issue identity is (kind, path), and
            // `clear_issues` matches on what the pipeline wrote.
            Ok(IssueRow {
                id: r.get(0)?,
                path: if path.is_empty() {
                    None
                } else {
                    Some(crate::winpath::for_display(&path).into_owned())
                },
                kind: r.get(2)?,
                message: r.get(3)?,
                first_seen_utc: r.get(4)?,
                last_seen_utc: r.get(5)?,
                occurrence_count: r.get::<_, i64>(6)?.max(1) as u64,
                recovery: None,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let active_recheck_issue = crate::scan_runtime::active_recheck_issue()?;
    for row in &mut rows {
        row.recovery = crate::issue_recovery::projection(
            conn,
            row.id,
            &row.kind,
            row.path.is_some(),
            active_recheck_issue,
        )?;
    }
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
        (year + 1, 1)
    } else {
        (year, mon + 1)
    };
    let start = display_tz
        .with_ymd_and_hms(year, mon, 1, 0, 0, 0)
        .earliest()
        .ok_or_else(|| format!("bad month start: {month}"))?
        .timestamp_millis();
    let end = display_tz
        .with_ymd_and_hms(next_year, next_mon, 1, 0, 0, 0)
        .earliest()
        .ok_or_else(|| format!("bad month end: {month}"))?
        .timestamp_millis();
    Ok(Some((start, end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_store;

    fn seeded() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-queries-")
            .tempdir()
            .unwrap();
        let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        (dir, conn)
    }

    fn projection() -> ItemProjectionContext {
        ItemProjectionContext {
            capabilities: crate::derived_state::WorkCapabilities {
                ffmpeg: true,
                video_snapshots_enabled: true,
                similarity_enabled: true,
                face_enabled: false,
                face_models: false,
                transcription_model: false,
                video_transcription_enabled: true,
                audio_transcription_enabled: true,
            },
            similarity_dirty: false,
        }
    }

    fn section_items(
        conn: &Connection,
        kind: &str,
        month: &str,
        display_tz: Tz,
        projection: ItemProjectionContext,
    ) -> Result<Vec<SectionItem>, String> {
        section_window(
            conn,
            kind,
            month,
            display_tz,
            SectionSort {
                order: SectionSortOrder::Time,
                desc: false,
            },
            0,
            MAX_SECTION_WINDOW_ITEMS,
            projection,
        )
        .map(|window| window.items)
    }

    fn utc_ms(y: i32, mo: u32, d: u32, h: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    // EXCEPTION (tests-folder conventions): the query-plan assertion must use
    // the private SQL builder that production executes; copying the SQL into
    // an integration test would let the test and implementation drift apart.
    #[test]
    fn a_month_section_seeks_the_logical_section_index() {
        let (_dir, conn) = seeded();
        let candidates = section_candidates_sql(false, true);
        let sql = format!(
            "EXPLAIN QUERY PLAN WITH candidates AS ({candidates}) \
             SELECT hash, path_id FROM candidates ORDER BY {} LIMIT 512 OFFSET 0",
            section_order_sql(SectionSort {
                order: SectionSortOrder::Time,
                desc: false,
            })
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let details: Vec<String> = stmt
            .query_map(
                rusqlite::params!["image", utc_ms(2026, 1, 1, 0), utc_ms(2026, 2, 1, 0)],
                |row| row.get(3),
            )
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            details
                .iter()
                .any(|line| line.contains("idx_logical_contents_section")),
            "section query lost its indexed month seek: {details:?}"
        );
        assert!(
            details.iter().all(|line| !line.starts_with("SCAN p")),
            "section query regressed to a whole paths scan: {details:?}"
        );
    }

    #[test]
    fn section_repair_seeks_directly_to_directory_facts() {
        let (_dir, conn) = seeded();
        let plans = [
            (
                section_dirs_sql(true),
                vec![
                    rusqlite::types::Value::Text("image".to_string()),
                    0.into(),
                    1.into(),
                ],
                vec!["idx_logical_contents_section", "idx_paths_content_hash"],
            ),
            (
                unhashed_other_section_dirs_sql(true),
                vec![0.into(), 1.into()],
                vec!["idx_paths_unhashed_other_section"],
            ),
        ];
        for (sql, params, expected_indexes) in plans {
            let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let details: Vec<String> = statement
                .query_map(rusqlite::params_from_iter(params), |row| row.get(3))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for expected in expected_indexes {
                assert!(
                    details.iter().any(|line| line.contains(expected)),
                    "section repair lost {expected}: {details:?}"
                );
            }
            assert!(
                details.iter().all(|line| !line.starts_with("SCAN p")),
                "section repair regressed to a whole paths scan: {details:?}"
            );
        }
    }

    #[test]
    fn sidebar_count_queries_seek_month_and_edge_indexes() {
        let (_dir, conn) = seeded();
        let plans = [
            (
                LOGICAL_MONTH_COUNT_SQL.to_string(),
                vec![
                    rusqlite::types::Value::Text("image".to_string()),
                    0.into(),
                    1.into(),
                ],
                "idx_logical_contents_section",
            ),
            (
                UNHASHED_OTHER_MONTH_COUNT_SQL.to_string(),
                vec![0.into(), 1.into()],
                "idx_paths_unhashed_other_section",
            ),
            (
                logical_edge_sql(false),
                vec![rusqlite::types::Value::Text("image".to_string())],
                "idx_logical_contents_section",
            ),
            (
                unhashed_other_edge_sql(false),
                Vec::new(),
                "idx_paths_unhashed_other_section",
            ),
        ];
        for (sql, params, expected_index) in plans {
            let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let details: Vec<String> = statement
                .query_map(rusqlite::params_from_iter(params), |row| row.get(3))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert!(
                details.iter().any(|line| line.contains(expected_index)),
                "sidebar count query lost {expected_index}: {details:?}"
            );
            assert!(
                details.iter().all(|line| !line.contains("USE TEMP B-TREE")),
                "sidebar count query introduced temporary sorting: {details:?}"
            );
        }
    }

    #[test]
    fn issues_page_seeks_oldest_diagnostics_without_sorting() {
        let (_dir, conn) = seeded();
        let mut statement = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {ISSUES_PAGE_SQL}"))
            .unwrap();
        let details: Vec<String> = statement
            .query_map([500], |row| row.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            details
                .iter()
                .any(|line| line.contains("idx_issues_first_seen")),
            "Issues page lost its oldest-first index: {details:?}"
        );
        assert!(
            details.iter().all(|line| !line.contains("USE TEMP B-TREE")),
            "Issues page reintroduced whole-table sorting: {details:?}"
        );
    }

    #[test]
    fn section_count_cache_tracks_sqlite_revision_and_timezone() {
        let (dir, writer) = seeded();
        writer
            .execute_batch(&format!(
                "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 1, 'image');
                 INSERT INTO paths
                   (abs_path, dir_path, file_name, kind, content_hash,
                    resolved_utc_ms, resolved_source)
                 VALUES ('/h1.jpg', '/', 'h1.jpg', 'image', 'h1', {boundary}, 'metadata');",
                boundary = utc_ms(2016, 3, 31, 22),
            ))
            .unwrap();
        let db = dir.path().join("index.sqlite3");
        let mut cache = SectionCountsCache::open(&db).unwrap();

        let (tokyo, first_recomputed) = cache.load(chrono_tz::Asia::Tokyo).unwrap();
        assert!(first_recomputed);
        assert_eq!(tokyo.images[0].month, "2016-04");
        let (_, repeated_recomputed) = cache.load(chrono_tz::Asia::Tokyo).unwrap();
        assert!(!repeated_recomputed);

        writer
            .execute_batch(&format!(
                "INSERT INTO contents (hash, byte_size, kind) VALUES ('h2', 1, 'image');
                 INSERT INTO paths
                   (abs_path, dir_path, file_name, kind, content_hash,
                    resolved_utc_ms, resolved_source)
                 VALUES ('/h2.jpg', '/', 'h2.jpg', 'image', 'h2', {earlier}, 'metadata');",
                earlier = utc_ms(2016, 3, 1, 0),
            ))
            .unwrap();
        let (updated, revision_recomputed) = cache.load(chrono_tz::Asia::Tokyo).unwrap();
        assert!(revision_recomputed);
        assert_eq!(
            updated.images.iter().map(|month| month.count).sum::<u64>(),
            2
        );

        let (utc, timezone_recomputed) = cache.load(chrono_tz::UTC).unwrap();
        assert!(timezone_recomputed);
        assert_eq!(
            utc.images,
            vec![MonthSection {
                month: "2016-03".into(),
                count: 2
            }]
        );
    }

    #[test]
    fn copies_collapse_to_one_logical_item_with_the_earliest_time() {
        let (_d, conn) = seeded();
        conn.execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 1, 'image');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/a/x.jpg', '/a', 'x.jpg', 'image', 'h1', {t1}, 'metadata');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/b/x.jpg', '/b', 'x.jpg', 'image', 'h1', {t2}, 'filesystem');",
            t1 = utc_ms(2016, 3, 5, 3),
            t2 = utc_ms(2020, 1, 1, 0),
        ))
        .unwrap();

        let counts = section_counts(&conn, chrono_tz::UTC).unwrap();
        assert_eq!(
            counts.images,
            vec![MonthSection {
                month: "2016-03".into(),
                count: 1
            }]
        );
    }

    #[test]
    fn display_timezone_decides_the_month_boundary() {
        let (_d, conn) = seeded();
        // 2016-03-31T22:00:00Z is already April 1st in JST (+09:00).
        conn.execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 1, 'image');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/a/y.jpg', '/a', 'y.jpg', 'image', 'h1', {t}, 'metadata');",
            t = utc_ms(2016, 3, 31, 22),
        ))
        .unwrap();

        let utc = section_counts(&conn, chrono_tz::UTC).unwrap();
        assert_eq!(utc.images[0].month, "2016-03");
        let jst = section_counts(&conn, chrono_tz::Asia::Tokyo).unwrap();
        assert_eq!(jst.images[0].month, "2016-04");
    }

    #[test]
    fn unhashed_other_files_count_individually_and_companions_never_appear() {
        let (_d, conn) = seeded();
        conn.execute_batch(&format!(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/doc.pdf', '/a', 'doc.pdf', 'other', {t}, 'filesystem');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/undatable.bin', '/a', 'undatable.bin', 'other', NULL, 'undated');
             INSERT INTO contents (hash, byte_size, kind) VALUES ('v1', 1, 'video');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/a/clip.mp4', '/a', 'clip.mp4', 'video', 'v1', {t}, 'metadata');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, companion_of, resolved_utc_ms, resolved_source)
               VALUES ('/a/clip.thm', '/a', 'clip.thm', 'companion', 3, {t}, 'filesystem');",
            t = utc_ms(2019, 7, 10, 12),
        ))
        .unwrap();

        let counts = section_counts(&conn, chrono_tz::UTC).unwrap();
        assert_eq!(counts.videos, vec![MonthSection { month: "2019-07".into(), count: 1 }]);
        assert_eq!(
            counts.others,
            vec![
                MonthSection { month: "2019-07".into(), count: 1 },
                MonthSection { month: "undated".into(), count: 1 },
            ]
        );
        assert!(counts.images.is_empty());
    }

    #[test]
    fn section_items_filters_by_month_and_reports_copies_and_thumbs() {
        let (_d, conn) = seeded();
        conn.execute_batch(&format!(
            "INSERT INTO contents
               (hash, byte_size, kind, width, height, derived_at_utc, derived_version)
               VALUES
               ('march', 1, 'image', 4000, 3000, '2026-08-08T00:00:00.000Z', {version});
             INSERT INTO contents (hash, byte_size, kind) VALUES ('april', 1, 'image');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/a/m.jpg', '/a', 'm.jpg', 'image', 'march', {mar}, 'metadata');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/b/m.jpg', '/b', 'm.jpg', 'image', 'march', {mar2}, 'filesystem');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/a/a.jpg', '/a', 'a.jpg', 'image', 'april', {apr}, 'metadata');",
            mar = utc_ms(2016, 3, 5, 3),
            mar2 = utc_ms(2016, 3, 6, 3),
            apr = utc_ms(2016, 4, 2, 3),
            version = crate::derived_state::DERIVE_VERSION,
        ))
        .unwrap();

        let march = section_items(&conn, "image", "2016-03", chrono_tz::UTC, projection()).unwrap();
        assert_eq!(march.len(), 1);
        assert_eq!(march[0].hash.as_deref(), Some("march"));
        assert_eq!(march[0].copy_count, 2);
        assert!(march[0].has_thumb);
        assert_eq!(march[0].resolved_utc_ms, Some(utc_ms(2016, 3, 5, 3)));

        let april = section_items(&conn, "image", "2016-04", chrono_tz::UTC, projection()).unwrap();
        assert_eq!(april.len(), 1);
        assert!(!april[0].has_thumb);

        assert!(section_items(&conn, "image", "2016-05", chrono_tz::UTC, projection())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn item_detail_lists_copies_and_companions() {
        let (_d, conn) = seeded();
        conn.execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind, width, height) VALUES ('h1', 42, 'image', 4000, 3000);
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/a/x.jpg', '/a', 'x.jpg', 'image', 'h1', {t}, 'metadata');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source)
               VALUES ('/b/x.jpg', '/b', 'x.jpg', 'image', 'h1', {t}, 'metadata');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, companion_of, resolved_utc_ms, resolved_source)
               VALUES ('/a/x.arw', '/a', 'x.arw', 'companion', 1, {t}, 'filesystem');",
            t = utc_ms(2016, 3, 5, 3),
        ))
        .unwrap();

        let detail = item_detail(&conn, Some("h1"), None).unwrap();
        assert_eq!(detail.file_name, "x.jpg");
        assert_eq!(detail.byte_size, Some(42));
        assert_eq!(detail.width, Some(4000));
        assert_eq!(detail.copy_paths, vec!["/a/x.jpg", "/b/x.jpg"]);
        assert_eq!(detail.companion_paths, vec!["/a/x.arw"]);
        assert_eq!(detail.resolved_source.as_deref(), Some("metadata"));
    }

    #[test]
    fn item_detail_hides_windows_verbatim_prefixes() {
        let (_d, conn) = seeded();
        conn.execute_batch(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 42, 'image');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
             VALUES (?1, ?2, 'x.jpg', 'image', 'h1')",
            [r"\\?\C:\photos\x.jpg", r"\\?\C:\photos"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, companion_of) \
             VALUES (?1, ?2, 'x.xmp', 'companion', 1)",
            [r"\\?\C:\photos\x.xmp", r"\\?\C:\photos"],
        )
        .unwrap();

        let detail = item_detail(&conn, Some("h1"), None).unwrap();
        assert_eq!(detail.copy_paths, vec![r"C:\photos\x.jpg"]);
        assert_eq!(detail.companion_paths, vec![r"C:\photos\x.xmp"]);
    }

    #[test]
    fn undated_sorts_last_and_months_sort_oldest_first() {
        let (_d, conn) = seeded();
        conn.execute_batch(&format!(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/new.bin', '/a', 'new.bin', 'other', {new}, 'filesystem');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/old.bin', '/a', 'old.bin', 'other', {old}, 'filesystem');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source)
               VALUES ('/a/none.bin', '/a', 'none.bin', 'other', NULL, 'undated');",
            new = utc_ms(2024, 12, 1, 0),
            old = utc_ms(2009, 1, 1, 0),
        ))
        .unwrap();

        let counts = section_counts(&conn, chrono_tz::UTC).unwrap();
        let months: Vec<&str> = counts.others.iter().map(|s| s.month.as_str()).collect();
        assert_eq!(months, vec!["2009-01", "2024-12", "undated"]);
    }
}

//! Durable result receipts for optional media analysis, plus the narrow
//! recovery policy that may reset those reconstructible results. This is not
//! a job queue: absence means pending, the coordinator remains the only
//! dispatcher, and only fixed, safe classes can be retried from Issues.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::preview::CachePaths;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkClass {
    Previews,
    Snapshots,
    Similarity,
    Faces,
    Transcripts,
}

impl WorkClass {
    pub(crate) const ALL: [Self; 5] = [
        Self::Previews,
        Self::Snapshots,
        Self::Similarity,
        Self::Faces,
        Self::Transcripts,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Previews => "previews",
            Self::Snapshots => "snapshots",
            Self::Similarity => "similarity",
            Self::Faces => "faces",
            Self::Transcripts => "transcripts",
        }
    }

    pub(crate) fn bit(self) -> u8 {
        1 << (self as u8)
    }

    pub(crate) fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.id() == id)
    }

    pub(crate) fn idle_only(self) -> bool {
        matches!(self, Self::Snapshots | Self::Faces | Self::Transcripts)
    }
}

#[derive(Clone, Copy)]
pub struct WorkCapabilities {
    pub ffmpeg: bool,
    pub face_enabled: bool,
    pub face_models: bool,
    pub transcripts: bool,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct WorkDebt {
    pub runnable: u64,
    pub blocked: u64,
    pub failed: u64,
    pub reason: Option<&'static str>,
    pub disabled: bool,
}

pub(crate) struct WorkDebts([WorkDebt; 5]);

impl WorkDebts {
    pub(crate) fn get(&self, class: WorkClass) -> WorkDebt {
        self.0[class as usize]
    }
}

struct DebtCounts {
    image_previews: u64,
    video_previews: u64,
    waiting_images: u64,
    preview_failures: u64,
    snapshots: u64,
    snapshot_failures: u64,
    faces: u64,
    face_failures: u64,
    transcripts: u64,
    transcript_failures: u64,
}

fn work_debt_sql(ffmpeg: bool) -> String {
    let (image_pending, _) = preview_pending_predicates(ffmpeg);
    let video_pending = video_preview_pending_predicate();
    format!(
        "SELECT
           COALESCE(SUM(l.kind = 'image' AND {image_pending}), 0),
           COALESCE(SUM(l.kind = 'video' AND {video_pending}), 0),
           COALESCE(SUM(l.kind = 'image' AND c.derived_at_utc = '{NEEDS_FFMPEG}'), 0),
           COALESCE(SUM(l.kind IN ('image', 'video') AND c.derived_at_utc = '{FAILED}'), 0),
           COALESCE(SUM(l.kind = 'video' AND c.strip_frames IS NULL
                        AND c.duration_ms IS NOT NULL
                        AND c.derived_at_utc NOT IN ('{FAILED}', '{NEEDS_FFMPEG}')), 0),
           COALESCE(SUM(l.kind = 'video' AND c.strip_frames < 0), 0),
           COALESCE(SUM(l.kind = 'image' AND r.face_state IS NULL
                        AND c.derived_at_utc IS NOT NULL
                        AND c.derived_at_utc NOT IN ('{FAILED}', '{NEEDS_FFMPEG}')), 0),
           COALESCE(SUM(r.face_state = '{FAILED}'), 0),
           COALESCE(SUM(l.kind = 'video' AND c.duration_ms IS NOT NULL
                        AND r.transcript_state IS NULL), 0),
           COALESCE(SUM(r.transcript_state = '{FAILED}'), 0)
         FROM logical_contents l
         JOIN contents c ON c.hash = l.content_hash
         LEFT JOIN analysis_receipts r ON r.content_hash = l.content_hash"
    )
}

/// Durable output debt for every fixed class. This is the sole owner of the
/// physical sentinel/receipt encoding; runtime and UI projection consume one
/// coherent aggregate over the maintained live-item projection rather than
/// repeatedly probing physical paths or inventing a second queue.
pub(crate) fn work_debts(
    conn: &Connection,
    capabilities: WorkCapabilities,
    similarity_dirty: bool,
) -> Result<WorkDebts, String> {
    let sql = work_debt_sql(capabilities.ffmpeg);
    let counts = conn
        .query_row(&sql, [], |row| {
            let count = |column| row.get::<_, i64>(column).map(|value| value.max(0) as u64);
            Ok(DebtCounts {
                image_previews: count(0)?,
                video_previews: count(1)?,
                waiting_images: count(2)?,
                preview_failures: count(3)?,
                snapshots: count(4)?,
                snapshot_failures: count(5)?,
                faces: count(6)?,
                face_failures: count(7)?,
                transcripts: count(8)?,
                transcript_failures: count(9)?,
            })
        })
        .map_err(|error| error.to_string())?;

    let previews = if capabilities.ffmpeg {
        WorkDebt {
            runnable: counts.image_previews + counts.video_previews,
            failed: counts.preview_failures,
            ..WorkDebt::default()
        }
    } else {
        let blocked = counts.video_previews + counts.waiting_images;
        WorkDebt {
            runnable: counts.image_previews,
            blocked,
            failed: counts.preview_failures,
            reason: (blocked > 0).then_some("Waiting for ffmpeg"),
            ..WorkDebt::default()
        }
    };
    let snapshots = if capabilities.ffmpeg {
        WorkDebt {
            runnable: counts.snapshots,
            failed: counts.snapshot_failures,
            ..WorkDebt::default()
        }
    } else {
        WorkDebt {
            blocked: counts.snapshots,
            failed: counts.snapshot_failures,
            reason: (counts.snapshots > 0).then_some("Waiting for ffmpeg"),
            ..WorkDebt::default()
        }
    };
    let similarity = WorkDebt {
        runnable: u64::from(similarity_dirty),
        ..WorkDebt::default()
    };
    let faces = if !capabilities.face_enabled {
        WorkDebt {
            disabled: true,
            reason: Some("Turn on face scoring in Settings"),
            ..WorkDebt::default()
        }
    } else if capabilities.face_models {
        WorkDebt {
            runnable: counts.faces,
            failed: counts.face_failures,
            ..WorkDebt::default()
        }
    } else {
        WorkDebt {
            blocked: counts.faces,
            failed: counts.face_failures,
            reason: (counts.faces > 0).then_some("Waiting for face models"),
            ..WorkDebt::default()
        }
    };
    let transcripts = if capabilities.transcripts {
        WorkDebt {
            runnable: counts.transcripts,
            failed: counts.transcript_failures,
            ..WorkDebt::default()
        }
    } else {
        WorkDebt {
            blocked: counts.transcripts,
            failed: counts.transcript_failures,
            reason: (counts.transcripts > 0)
                .then_some("Waiting for ffmpeg and transcription model"),
            ..WorkDebt::default()
        }
    };
    Ok(WorkDebts([
        previews,
        snapshots,
        similarity,
        faces,
        transcripts,
    ]))
}

// EXCEPTION (tests-folder convention): this pins the private aggregate SQL
// owned here, so the shipped projection and its structural assertion cannot
// drift into different queries.
#[cfg(test)]
mod debt_query_tests {
    use super::*;

    #[test]
    fn debt_snapshot_scans_one_logical_projection_and_never_probes_paths() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-debt-plan-")
            .tempdir()
            .unwrap();
        let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let mut statement = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", work_debt_sql(true)))
            .unwrap();
        let details: Vec<String> = statement
            .query_map([], |row| row.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert_eq!(
            details
                .iter()
                .filter(|line| line.starts_with("SCAN "))
                .count(),
            1,
            "debt projection must aggregate one source scan: {details:?}"
        );
        assert!(
            details.iter().any(|line| line.contains("logical_contents")),
            "debt projection lost the maintained live-item truth: {details:?}"
        );
        assert!(
            details.iter().all(|line| !line.contains("paths")),
            "debt projection regressed to physical-path probes: {details:?}"
        );
    }
}

pub const FACE_ERROR: &str = "face-score-error";
pub const PREVIEW_ERROR: &str = "decode-error";
pub const TRANSCRIPT_ERROR: &str = "transcription-error";
pub const VIDEO_POSTER_ERROR: &str = "video-poster-error";
pub const VIDEO_STRIP_ERROR: &str = "video-strip-error";

pub const READY: &str = "ready";
pub const READY_TEXT: &str = "ready-text";
pub const READY_EMPTY: &str = "ready-empty";
pub const FAILED: &str = "failed";
pub const NEEDS_FFMPEG: &str = "needs-ffmpeg";
pub const DERIVE_VERSION: i64 = 3;
const STRIP_FAILED: i64 = -1;
pub const SNAPSHOT_CANDIDATE_PAGE_SIZE: usize = 32;
pub const FACE_CANDIDATE_PAGE_SIZE: usize = 32;
pub const TRANSCRIPT_CANDIDATE_PAGE_SIZE: usize = 64;

pub(crate) fn preview_pending_predicates(ffmpeg: bool) -> (String, String) {
    let stale = format!(
        "c.derived_version < {DERIVE_VERSION} \
         AND c.derived_at_utc NOT IN ('{FAILED}', '{NEEDS_FFMPEG}')",
    );
    let image = if ffmpeg {
        format!(
            "(c.derived_at_utc IS NULL OR c.derived_at_utc = '{}' OR ({stale}))",
            NEEDS_FFMPEG,
        )
    } else {
        format!("(c.derived_at_utc IS NULL OR ({stale}))")
    };
    let video = if ffmpeg {
        video_preview_pending_predicate()
    } else {
        "0".to_string()
    };
    (image, video)
}

fn video_preview_pending_predicate() -> String {
    format!(
        "(c.derived_at_utc IS NULL OR \
         (c.derived_version < {DERIVE_VERSION} AND c.derived_at_utc != '{FAILED}'))"
    )
}

pub(crate) fn preview_available_predicate(content_alias: &str) -> String {
    format!(
        "({content_alias}.derived_at_utc IS NOT NULL AND \
         {content_alias}.derived_at_utc NOT IN ('{FAILED}', '{NEEDS_FFMPEG}'))"
    )
}

pub(crate) fn image_candidates(
    conn: &Connection,
    ffmpeg: bool,
    limit: Option<usize>,
    only_hash: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let (pending, _) = preview_pending_predicates(ffmpeg);
    let mut statement = conn
        .prepare(&format!(
            "SELECT l.content_hash, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             WHERE l.kind = 'image' AND {pending} AND p.missing = 0 \
               AND (?1 IS NULL OR l.content_hash = ?1) \
             ORDER BY l.content_hash LIMIT ?2"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![only_hash, limit.map_or(i64::MAX, |value| value as i64)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub(crate) fn video_candidates(
    conn: &Connection,
    ffmpeg: bool,
    limit: Option<usize>,
    only_hash: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let (_, pending) = preview_pending_predicates(ffmpeg);
    let mut statement = conn
        .prepare(&format!(
            "SELECT l.content_hash, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             WHERE l.kind = 'video' AND {pending} AND p.missing = 0 \
               AND (?1 IS NULL OR l.content_hash = ?1) \
             ORDER BY l.content_hash LIMIT ?2"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![only_hash, limit.map_or(i64::MAX, |value| value as i64)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn strip_candidates(
    conn: &Connection,
    after_hash: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, i64, String)>, String> {
    let mut statement = conn
        .prepare(
            "SELECT c.hash, c.duration_ms, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             WHERE l.kind = 'video' AND c.strip_frames IS NULL \
               AND c.duration_ms IS NOT NULL \
               AND c.derived_at_utc IS NOT NULL \
               AND c.derived_at_utc NOT IN (?1, ?2) \
               AND l.content_hash > ?3 AND p.missing = 0 \
             ORDER BY l.content_hash LIMIT ?4",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![FAILED, NEEDS_FFMPEG, after_hash.unwrap_or(""), limit as i64],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn face_candidates(
    conn: &Connection,
    after_hash: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, String)>, String> {
    let mut statement = conn
        .prepare(
            "SELECT c.hash, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
             WHERE l.kind = 'image' AND r.face_state IS NULL \
               AND c.derived_at_utc IS NOT NULL \
               AND c.derived_at_utc NOT IN (?1, ?2) \
               AND l.content_hash > ?3 AND p.missing = 0 \
             ORDER BY l.content_hash LIMIT ?4",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![
                FAILED,
                NEEDS_FFMPEG,
                after_hash.unwrap_or(""),
                limit as i64
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn transcript_candidates(
    conn: &Connection,
    after_hash: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, String)>, String> {
    let mut statement = conn
        .prepare(
            "SELECT c.hash, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
             WHERE l.kind = 'video' AND c.duration_ms IS NOT NULL \
               AND r.transcript_state IS NULL AND p.missing = 0 \
               AND l.content_hash > ?1 \
             ORDER BY l.content_hash LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![after_hash.unwrap_or(""), limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssueRecovery {
    pub label: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptResult {
    pub status: &'static str,
    pub text: Option<String>,
    pub message: Option<String>,
}

pub fn transcript_result(
    conn: &Connection,
    cache: &CachePaths,
    hash: &str,
) -> Result<TranscriptResult, String> {
    let state: Option<String> = conn
        .query_row(
            "SELECT transcript_state FROM analysis_receipts WHERE content_hash = ?1",
            [hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .flatten();
    match state.as_deref() {
        Some(READY_TEXT | READY_EMPTY) => match std::fs::read_to_string(cache.transcript(hash)) {
            Ok(text) => Ok(TranscriptResult {
                status: READY,
                text: Some(text),
                message: None,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                conn.execute(
                    "UPDATE analysis_receipts SET transcript_state = NULL,
                         transcript_updated_at_utc = NULL WHERE content_hash = ?1",
                    [hash],
                )
                .map_err(|error| error.to_string())?;
                Ok(TranscriptResult {
                    status: "pending",
                    text: None,
                    message: None,
                })
            }
            Err(error) => Err(format!("transcript cache read failed: {error}")),
        },
        Some(FAILED) => {
            let message = conn
                .query_row(
                    "SELECT i.message FROM issues i JOIN paths p ON p.abs_path = i.path
                     WHERE i.kind = ?1 AND p.content_hash = ?2 LIMIT 1",
                    params![TRANSCRIPT_ERROR, hash],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?
                .flatten();
            Ok(TranscriptResult {
                status: FAILED,
                text: None,
                message,
            })
        }
        _ => match std::fs::read_to_string(cache.transcript(hash)) {
            Ok(text) => {
                let path: String = conn
                    .query_row(
                        "SELECT abs_path FROM paths
                         WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
                        [hash],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                record_transcript_success(conn, hash, &path, !text.trim().is_empty())?;
                Ok(TranscriptResult {
                    status: READY,
                    text: Some(text),
                    message: None,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(TranscriptResult {
                status: "pending",
                text: None,
                message: None,
            }),
            Err(error) => Err(format!("transcript cache read failed: {error}")),
        },
    }
}

pub fn record_preview_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    width: u32,
    height: u32,
    sharpness: f64,
    phash: u64,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            &format!(
                "UPDATE contents SET width = COALESCE(width, ?2), \
                 height = COALESCE(height, ?3), sharpness = ?4, phash = ?5, \
                 derived_at_utc = ?6, derived_version = {DERIVE_VERSION} \
                 WHERE hash = ?1"
            ),
            params![
                hash,
                width,
                height,
                sharpness,
                phash as i64,
                crate::logging::now_iso_millis()
            ],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[PREVIEW_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_preview_blocked(conn: &Connection, hash: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE contents SET derived_at_utc = ?2 WHERE hash = ?1",
        params![hash, NEEDS_FFMPEG],
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn record_content_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    issue_kind: &str,
    message: &str,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET derived_at_utc = ?2 WHERE hash = ?1",
            params![hash, FAILED],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::upsert_issue(&transaction, Some(path), issue_kind, message)?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_preview_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    record_content_failure(conn, hash, path, PREVIEW_ERROR, message)
}

pub fn record_poster_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    duration_ms: u64,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            &format!(
                "UPDATE contents SET duration_ms = COALESCE(duration_ms, ?2), \
                 derived_at_utc = ?3, derived_version = {DERIVE_VERSION} WHERE hash = ?1"
            ),
            params![hash, duration_ms as i64, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[VIDEO_POSTER_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_poster_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    record_content_failure(conn, hash, path, VIDEO_POSTER_ERROR, message)
}

pub fn record_strip_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    frame_count: u32,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET strip_frames = ?2 WHERE hash = ?1",
            params![hash, frame_count as i64],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[VIDEO_STRIP_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_strip_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET strip_frames = ?2 WHERE hash = ?1",
            params![hash, STRIP_FAILED],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::upsert_issue(&transaction, Some(path), VIDEO_STRIP_ERROR, message)?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_face_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    score: f64,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET face_score = ?2 WHERE hash = ?1",
            params![hash, score],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO analysis_receipts
               (content_hash, face_state, face_updated_at_utc)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (content_hash) DO UPDATE SET
               face_state = excluded.face_state,
               face_updated_at_utc = excluded.face_updated_at_utc",
            params![hash, READY, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[FACE_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_face_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET face_score = NULL WHERE hash = ?1",
            [hash],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO analysis_receipts
               (content_hash, face_state, face_updated_at_utc)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (content_hash) DO UPDATE SET
               face_state = excluded.face_state,
               face_updated_at_utc = excluded.face_updated_at_utc",
            params![hash, FAILED, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::upsert_issue(&transaction, Some(path), FACE_ERROR, message)?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_transcript_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    has_text: bool,
) -> Result<(), String> {
    let state = if has_text { READY_TEXT } else { READY_EMPTY };
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO analysis_receipts
               (content_hash, transcript_state, transcript_updated_at_utc)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (content_hash) DO UPDATE SET
               transcript_state = excluded.transcript_state,
               transcript_updated_at_utc = excluded.transcript_updated_at_utc",
            params![hash, state, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[TRANSCRIPT_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_transcript_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO analysis_receipts
               (content_hash, transcript_state, transcript_updated_at_utc)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (content_hash) DO UPDATE SET
               transcript_state = excluded.transcript_state,
               transcript_updated_at_utc = excluded.transcript_updated_at_utc",
            params![hash, FAILED, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::upsert_issue(&transaction, Some(path), TRANSCRIPT_ERROR, message)?;
    transaction.commit().map_err(|error| error.to_string())
}

fn content_hash_for_issue(
    conn: &Connection,
    issue_id: i64,
) -> Result<Option<(String, String, String)>, String> {
    conn.query_row(
        "SELECT i.kind, i.path, p.content_hash
         FROM issues i
         JOIN paths p ON p.abs_path = i.path AND p.missing = 0
         WHERE i.id = ?1 AND p.content_hash IS NOT NULL
         LIMIT 1",
        [issue_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(|error| error.to_string())
}

/// Resets only a reconstructible output named by the current issue. The issue
/// remains visible until that output succeeds and clears it.
pub fn retry_issue(conn: &Connection, issue_id: i64) -> Result<bool, String> {
    let Some((kind, _path, hash)) = content_hash_for_issue(conn, issue_id)? else {
        return Ok(false);
    };
    let changed = match kind.as_str() {
        PREVIEW_ERROR | VIDEO_POSTER_ERROR => conn.execute(
            "UPDATE contents SET derived_at_utc = NULL
             WHERE hash = ?1 AND derived_at_utc IS NOT NULL",
            [&hash],
        ),
        VIDEO_STRIP_ERROR => conn.execute(
            "UPDATE contents SET strip_frames = NULL
             WHERE hash = ?1 AND strip_frames IS NOT NULL",
            [&hash],
        ),
        FACE_ERROR => conn.execute(
            "UPDATE analysis_receipts SET face_state = NULL,
                 face_updated_at_utc = NULL
             WHERE content_hash = ?1 AND face_state IS NOT NULL",
            [&hash],
        ),
        TRANSCRIPT_ERROR => conn.execute(
            "UPDATE analysis_receipts SET transcript_state = NULL,
                 transcript_updated_at_utc = NULL
             WHERE content_hash = ?1 AND transcript_state IS NOT NULL",
            [&hash],
        ),
        _ => return Ok(false),
    }
    .map_err(|error| error.to_string())?;
    Ok(changed > 0)
}

pub fn retry_all(conn: &Connection) -> Result<u64, String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let mut retried = transaction
        .execute(
            "UPDATE contents SET derived_at_utc = NULL \
             WHERE derived_at_utc IS NOT NULL \
               AND EXISTS (SELECT 1 FROM paths p JOIN issues i ON i.path = p.abs_path \
                           WHERE p.content_hash = contents.hash AND p.missing = 0 \
                             AND i.kind IN (?1, ?2))",
            params![PREVIEW_ERROR, VIDEO_POSTER_ERROR],
        )
        .map_err(|error| error.to_string())? as u64;
    retried += transaction
        .execute(
            "UPDATE contents SET strip_frames = NULL \
             WHERE strip_frames IS NOT NULL \
               AND EXISTS (SELECT 1 FROM paths p JOIN issues i ON i.path = p.abs_path \
                           WHERE p.content_hash = contents.hash AND p.missing = 0 \
                             AND i.kind = ?1)",
            [VIDEO_STRIP_ERROR],
        )
        .map_err(|error| error.to_string())? as u64;
    retried += transaction
        .execute(
            "UPDATE analysis_receipts SET face_state = NULL, face_updated_at_utc = NULL \
             WHERE face_state IS NOT NULL \
               AND EXISTS (SELECT 1 FROM paths p JOIN issues i ON i.path = p.abs_path \
                           WHERE p.content_hash = analysis_receipts.content_hash \
                             AND p.missing = 0 AND i.kind = ?1)",
            [FACE_ERROR],
        )
        .map_err(|error| error.to_string())? as u64;
    retried += transaction
        .execute(
            "UPDATE analysis_receipts \
             SET transcript_state = NULL, transcript_updated_at_utc = NULL \
             WHERE transcript_state IS NOT NULL \
               AND EXISTS (SELECT 1 FROM paths p JOIN issues i ON i.path = p.abs_path \
                           WHERE p.content_hash = analysis_receipts.content_hash \
                             AND p.missing = 0 AND i.kind = ?1)",
            [TRANSCRIPT_ERROR],
        )
        .map_err(|error| error.to_string())? as u64;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(retried)
}

pub fn issue_recovery(conn: &Connection, issue_id: i64) -> Result<Option<IssueRecovery>, String> {
    let Some((kind, _path, hash)) = content_hash_for_issue(conn, issue_id)? else {
        return Ok(None);
    };
    let queued = match kind.as_str() {
        PREVIEW_ERROR | VIDEO_POSTER_ERROR => conn
            .query_row(
                "SELECT derived_at_utc IS NULL FROM contents WHERE hash = ?1",
                [&hash],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?,
        VIDEO_STRIP_ERROR => conn
            .query_row(
                "SELECT strip_frames IS NULL FROM contents WHERE hash = ?1",
                [&hash],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?,
        FACE_ERROR => conn
            .query_row(
                "SELECT face_state IS NULL FROM analysis_receipts WHERE content_hash = ?1",
                [&hash],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?,
        TRANSCRIPT_ERROR => conn
            .query_row(
                "SELECT transcript_state IS NULL FROM analysis_receipts WHERE content_hash = ?1",
                [&hash],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?,
        _ => return Ok(None),
    };
    Ok(queued.map(|queued| IssueRecovery {
        label: "Retry",
        status: if queued { "queued" } else { "available" },
    }))
}

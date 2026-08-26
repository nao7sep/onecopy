//! Durable result receipts for optional media analysis, plus the narrow
//! recovery policy that may reset those reconstructible results. This is not
//! a job queue: absence means pending, the coordinator remains the only
//! dispatcher, and only fixed, safe classes can be retried from Issues.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::preview::CachePaths;

pub const FACE_ERROR: &str = "face-score-error";
pub const PREVIEW_ERROR: &str = "decode-error";
pub const TRANSCRIPT_ERROR: &str = "transcription-error";
pub const VIDEO_POSTER_ERROR: &str = "video-poster-error";
pub const VIDEO_STRIP_ERROR: &str = "video-strip-error";

pub const READY: &str = "ready";
pub const READY_TEXT: &str = "ready-text";
pub const READY_EMPTY: &str = "ready-empty";
pub const FAILED: &str = "failed";

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

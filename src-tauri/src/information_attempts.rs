//! File-information attempt receipts are independent of diagnostic visibility.

use rusqlite::{params, Connection};

#[derive(Clone, Copy)]
pub enum Stage {
    Identity,
    Metadata,
}

impl Stage {
    fn column(self) -> &'static str {
        match self {
            Self::Identity => "hash_attempt_failed",
            Self::Metadata => "metadata_attempt_failed",
        }
    }

    pub fn issue_kind(self) -> &'static str {
        match self {
            Self::Identity => crate::scanner::READ_ERROR,
            Self::Metadata => crate::scanner::METADATA_READ_ERROR,
        }
    }

    fn presentation(self) -> &'static str {
        match self {
            Self::Identity => "OneCopy could not read this file to identify its content. Check that it is accessible, then recheck its section.",
            Self::Metadata => "OneCopy could not read this file's metadata. Check that it is accessible, then recheck its section.",
        }
    }
}

pub fn failed(
    conn: &Connection,
    id: i64,
    path: &str,
    stage: Stage,
    error: &str,
) -> Result<(), String> {
    crate::logging::warn(
        "file information failed",
        serde_json::json!({
            "path": path, "kind": stage.issue_kind(), "error": { "message": error }
        }),
    );
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            &format!("UPDATE paths SET {} = 1 WHERE id = ?1", stage.column()),
            [id],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::upsert_issue(
        &transaction,
        Some(path),
        stage.issue_kind(),
        stage.presentation(),
    )?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn reset_library(conn: &Connection) -> Result<u64, String> {
    conn.execute(
        "UPDATE paths SET hash_attempt_failed = 0, metadata_attempt_failed = 0
         WHERE hash_attempt_failed = 1 OR metadata_attempt_failed = 1",
        [],
    )
    .map(|count| count as u64)
    .map_err(|error| error.to_string())
}

/// Logical members and their local companions use the logical section's date. Unidentified paths have
/// no logical row yet, so their own kind and completed date evidence scope
/// their attempt without admitting other contents in the same directory.
pub fn reset_section(
    conn: &Connection,
    kind: &str,
    bounds: Option<(i64, i64)>,
) -> Result<u64, String> {
    let members = section_paths(kind, bounds)?;
    conn.execute(
        &format!(
            "UPDATE paths SET hash_attempt_failed = 0, metadata_attempt_failed = 0
         WHERE (hash_attempt_failed = 1 OR metadata_attempt_failed = 1)
           AND id IN ({members})"
        ),
        params![
            kind,
            bounds.map(|(start, _)| start),
            bounds.map(|(_, end)| end)
        ],
    )
    .map(|count| count as u64)
    .map_err(|error| error.to_string())
}

pub(crate) fn section_paths(kind: &str, bounds: Option<(i64, i64)>) -> Result<String, String> {
    if !matches!(kind, "image" | "video" | "other") {
        return Err(format!("bad section kind: {kind}"));
    }
    if bounds.is_some_and(|(start, end)| start >= end) {
        return Err("section date range must be increasing".to_string());
    }
    let dates = if bounds.is_some() {
        "resolved_utc_ms >= ?2 AND resolved_utc_ms < ?3"
    } else {
        "resolved_utc_ms IS NULL AND ?2 IS NULL AND ?3 IS NULL"
    };
    Ok(format!(
        "WITH section_contents AS (
           SELECT content_hash FROM logical_contents WHERE kind = ?1 AND {dates}
         )
         SELECT id FROM paths WHERE missing = 0
           AND (content_hash IN (SELECT content_hash FROM section_contents)
                OR companion_of IN (SELECT id FROM paths WHERE content_hash IN (SELECT content_hash FROM section_contents))
                OR (content_hash IS NULL AND companion_of IS NULL
                    AND CASE WHEN kind IN ('image', 'video') THEN kind ELSE 'other' END = ?1
                    AND {dates}))"
    ))
}

//! Explicit admission retires diagnostics and reopens failed receipts atomically.
//! Ordinary database reads and section navigation never call these boundaries.

use crate::{derived_state, index_store, information_attempts};
use rusqlite::{params, Connection};

pub fn begin_run(conn: &Connection) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    index_store::begin_issue_run(&transaction)?;
    transaction.execute("UPDATE paths SET visibility_checked = 0 WHERE visibility_checked = -1", [])
        .map_err(|error| error.to_string())?;
    information_attempts::reset_library(&transaction)?;
    derived_state::reset_failed_outputs_in_transaction(
        &transaction,
        derived_state::FailedOutputScope::Library,
    )?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn recheck_section(
    conn: &Connection,
    kind: &str,
    bounds: Option<(i64, i64)>,
) -> Result<u64, String> {
    let members = information_attempts::section_paths(kind, bounds)?;
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            &format!(
                "UPDATE issues SET closure = 'rechecked', closed_at_utc = ?4
         WHERE closed_at_utc IS NULL
           AND path IN (SELECT abs_path FROM paths WHERE id IN ({members}))
           AND kind IN (?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ),
            params![
                kind,
                bounds.map(|(start, _)| start),
                bounds.map(|(_, end)| end),
                crate::logging::now_iso_millis(),
                crate::scanner::READ_ERROR,
                crate::scanner::METADATA_READ_ERROR,
                derived_state::PREVIEW_ERROR,
                derived_state::VIDEO_POSTER_ERROR,
                derived_state::VIDEO_STRIP_ERROR,
                derived_state::FACE_ERROR,
                derived_state::TRANSCRIPT_ERROR,
            ],
        )
        .map_err(|error| error.to_string())?;
    information_attempts::reset_section(&transaction, kind, bounds)?;
    let reopened = derived_state::reset_failed_outputs_in_transaction(
        &transaction,
        derived_state::FailedOutputScope::Section { kind, bounds },
    )?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(reopened)
}

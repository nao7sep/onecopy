//! Explicit admission retires diagnostics and reopens failed outputs atomically.
//! Ordinary database reads and section navigation never call these boundaries.

use crate::{derived_state, index_store, information_attempts};
use rusqlite::{params, Connection};

pub fn begin_run(conn: &Connection) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction.execute("UPDATE paths SET visibility_checked = 0 WHERE visibility_checked = -1", [])
        .map_err(|error| error.to_string())?;
    information_attempts::reset_library(&transaction)?;
    derived_state::reset_failed_outputs_in_transaction(
        &transaction,
        derived_state::FailedOutputScope::Library,
    )?;
    crate::records::commit(transaction).map_err(|error| error.to_string())
}

pub fn recheck_section(
    conn: &Connection,
    kind: crate::queries::SectionKind,
    bounds: Option<(i64, i64)>,
) -> Result<u64, String> {
    let members = information_attempts::section_paths(bounds)?;
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    index_store::close_issues(
        &transaction,
        "rechecked",
        &format!(
            "path IN (SELECT abs_path FROM paths WHERE id IN ({members}))
             AND kind IN (?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        ),
        params![
            kind,
            bounds.map(|(start, _)| start),
            bounds.map(|(_, end)| end),
            crate::scanner::READ_ERROR,
            crate::scanner::METADATA_READ_ERROR,
            derived_state::PREVIEW_ERROR,
            derived_state::VIDEO_POSTER_ERROR,
            derived_state::VIDEO_STRIP_ERROR,
            derived_state::FACE_ERROR,
            derived_state::TRANSCRIPT_ERROR,
        ],
    )?;
    information_attempts::reset_section(&transaction, kind, bounds)?;
    let reopened = derived_state::reset_failed_outputs_in_transaction(
        &transaction,
        derived_state::FailedOutputScope::Section { kind, bounds },
    )?;
    crate::records::commit(transaction).map_err(|error| error.to_string())?;
    Ok(reopened)
}

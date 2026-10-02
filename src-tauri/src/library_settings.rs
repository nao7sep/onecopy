//! Brings the index to the saved library settings: the visibility policy and
//! the settings that dates and companion relationships are resolved with.
//! `config.json` alone owns them.
//!
//! The index stamps which settings it was projected with, so applying is
//! idempotent and a difference is durable index debt. A Settings apply that
//! cannot be admitted in time leaves that debt for the file-information owner,
//! which applies it at its next turn, including after a restart.

use rusqlite::{params, Connection, OptionalExtension};

use crate::scanner::{ScanProgress, ScanSettings};
use crate::visibility::Policy;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolutionPolicy {
    default_timezone: String,
    good_range_start_year: i32,
    pairing_enabled: bool,
}

impl ResolutionPolicy {
    fn from_settings(settings: &ScanSettings) -> Self {
        Self {
            default_timezone: settings.resolution.default_timezone.name().to_string(),
            good_range_start_year: settings.resolution.good_range_start_year,
            pairing_enabled: settings.pairing_enabled,
        }
    }
}

fn recorded(conn: &Connection) -> Result<Option<ResolutionPolicy>, String> {
    conn.query_row(
        "SELECT default_timezone, good_range_start_year, pairing_enabled FROM library_choices
         WHERE default_timezone IS NOT NULL",
        [],
        |row| {
            Ok(ResolutionPolicy {
                default_timezone: row.get(0)?,
                good_range_start_year: row.get(1)?,
                pairing_enabled: row.get::<_, i64>(2)? != 0,
            })
        },
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn record(conn: &Connection, policy: &ResolutionPolicy) -> Result<(), String> {
    conn.execute(
        "UPDATE library_choices SET default_timezone = ?1, good_range_start_year = ?2,
           pairing_enabled = ?3",
        params![
            policy.default_timezone,
            policy.good_range_start_year,
            policy.pairing_enabled
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// An index that has never recorded its resolution settings was resolved with
/// the settings it is opened with. Called at launch, before any Settings
/// apply, so a later difference is always a real change.
pub fn adopt_unrecorded(conn: &Connection, settings: &ScanSettings) -> Result<(), String> {
    if recorded(conn)?.is_none() {
        record(conn, &ResolutionPolicy::from_settings(settings))?;
    }
    Ok(())
}

/// Whether the index still owes an apply of these settings.
pub fn owed(conn: &Connection, settings: &ScanSettings, visibility: &Policy) -> Result<bool, String> {
    if !crate::visibility_index::policy_applied(conn, visibility)? {
        return Ok(true);
    }
    Ok(recorded(conn)?
        .is_some_and(|recorded| recorded != ResolutionPolicy::from_settings(settings)))
}

/// Applies these settings to the index and returns how many dates it
/// resolved again. An interrupted apply records nothing, so it stays owed and
/// runs again from the start.
pub fn apply(
    conn: &Connection,
    settings: &ScanSettings,
    visibility: &Policy,
    progress: &dyn Fn(ScanProgress),
) -> Result<u64, String> {
    crate::visibility_index::apply_policy(conn, visibility)?;
    let wanted = ResolutionPolicy::from_settings(settings);
    match recorded(conn)? {
        Some(current) if current == wanted => return Ok(0),
        None => {
            record(conn, &wanted)?;
            return Ok(0);
        }
        Some(_) => {}
    }
    // Resolution rows carry their own resumable debt. Pairing is an atomic
    // projection, so retain the coarse dirty-root receipt across the whole
    // rebuild; cancellation before publication makes a later index repair
    // retry it.
    let stats = crate::scanner::with_scoped_index_repair(conn, &settings.source_dirs, || {
        crate::scanner::re_resolve_all_with_progress(
            conn,
            &settings.resolution,
            settings.pairing_enabled,
            progress,
        )
    })?;
    record(conn, &wanted)?;
    Ok(stats.resolved)
}

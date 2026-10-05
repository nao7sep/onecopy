//! Every store's format version, held in one place (store-recovery
//! conventions). Each format has its own number; the data is not durable yet,
//! so every one stays at 1 and a format change edits its format in place. A
//! store without its marker is unreadable: no version is inferred from shape.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value as JsonValue;

/// `index.sqlite3`, as `PRAGMA user_version`.
pub const INDEX: i64 = 1;
/// The per-row `contents.derived_version` of `index.sqlite3`: a row whose
/// derived results carry an older version is derived again.
pub const DERIVE_VERSION: i64 = 1;
/// `records.sqlite3`, as `PRAGMA user_version`.
pub const RECORDS: i64 = 1;
/// `backups.sqlite3`, as `PRAGMA user_version`.
pub const BACKUPS: i64 = 1;
/// `config.json`.
pub const CONFIG: i64 = 1;
/// `state.json`.
pub const STATE: i64 = 1;
/// `window.json`, `preview-window.json` and `records-window.json`, which
/// share one placement format.
pub const WINDOW_PLACEMENT: i64 = 1;
/// `source-volumes.json`.
pub const SOURCE_VOLUMES: i64 = 1;
/// `dependencies.json`.
pub const DEPENDENCIES: i64 = 1;
/// The version sidecar beside the managed ffmpeg (`bin/ffmpeg.json`).
pub const FFMPEG_VERSION_SIDECAR: i64 = 1;
/// The identity sidecar beside each managed model (`models/<file>.json`).
pub const MODEL_IDENTITY: i64 = 1;
/// `manifest.json` inside each archive under `backups/`.
pub const ARCHIVE_MANIFEST: i64 = 1;
/// Every line of a deleted-files day folder's `manifest.jsonl`.
pub const DELETED_FILES_MANIFEST: i64 = 1;

/// The marker's key in a JSON document or line.
pub const JSON_KEY: &str = "formatVersion";

/// The version a JSON document records. A missing marker, or one that is
/// not a positive integer, makes the document unreadable.
pub fn json_version(document: &JsonValue) -> Result<i64, String> {
    let marker = document
        .get(JSON_KEY)
        .ok_or_else(|| format!("{JSON_KEY} is missing"))?;
    marker
        .as_i64()
        .filter(|version| *version >= 1)
        .ok_or_else(|| format!("{JSON_KEY} is not a positive integer: {marker}"))
}

/// Removes the marker from a JSON object and returns the version it
/// recorded, so readers see only the store's own keys.
pub fn take_json_version(document: &mut JsonValue) -> Result<i64, String> {
    let version = json_version(document)?;
    if let Some(object) = document.as_object_mut() {
        object.remove(JSON_KEY);
    }
    Ok(version)
}

/// Sets the marker on a JSON object.
pub fn stamp_json(document: &mut JsonValue, version: i64) -> Result<(), String> {
    document
        .as_object_mut()
        .ok_or_else(|| "a versioned JSON store must be an object".to_string())?
        .insert(JSON_KEY.to_string(), JsonValue::from(version));
    Ok(())
}

/// What a SQLite store's `user_version` says to this build.
#[derive(Debug, PartialEq, Eq)]
pub enum SqliteMarker {
    /// No marker and no schema: a store being created.
    New,
    /// The version this build reads.
    Current,
    /// A schema without its marker: unreadable.
    Missing,
    Newer(NewerStore),
}

/// Reads a SQLite store's marker. SQLite's `user_version` is 0 until set, so
/// 0 over an existing schema is a missing marker, and 0 over an empty file is
/// a store not created yet.
pub fn sqlite_marker(connection: &Connection, path: &Path, supported: i64) -> Result<SqliteMarker, String> {
    let recorded: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("read the format version: {error}"))?;
    if recorded > 0 {
        return Ok(match NewerStore::check(path, recorded, supported) {
            Some(newer) => SqliteMarker::Newer(newer),
            None => SqliteMarker::Current,
        });
    }
    let objects: i64 = connection
        .query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row.get(0))
        .map_err(|error| format!("read the schema: {error}"))?;
    Ok(if objects == 0 { SqliteMarker::New } else { SqliteMarker::Missing })
}

/// The message for a SQLite store whose schema carries no marker.
pub fn missing_marker(path: &Path) -> String {
    format!("{} has no format version and cannot be read", path.display())
}

/// Whether the SQLite store at `path` was written by a newer OneCopy, read
/// without writing anything. An absent file is not.
pub fn sqlite_newer(path: &Path, supported: i64) -> Result<Option<NewerStore>, String> {
    match std::fs::symlink_metadata(path) { // data root
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    Ok(match sqlite_marker(&connection, path, supported)? {
        SqliteMarker::Newer(newer) => Some(newer),
        _ => None,
    })
}

/// A store written by a newer OneCopy: intact data this build cannot read.
/// It is reported by name and left exactly in place, never quarantined,
/// reset or written to, so the version that wrote it can still read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewerStore {
    /// The store's file name, `index.sqlite3`.
    pub file: String,
    /// The store's full path, verbatim.
    pub path: String,
    /// The version the store records.
    pub version: i64,
    /// The newest version this build reads.
    pub supported: i64,
}

impl NewerStore {
    /// The store at `path` when its recorded `version` is newer than
    /// `supported`.
    pub fn check(path: &Path, version: i64, supported: i64) -> Option<Self> {
        (version > supported).then(|| Self {
            file: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            path: path.to_string_lossy().into_owned(),
            version,
            supported,
        })
    }
}

impl std::fmt::Display for NewerStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} was written by a newer OneCopy (format version {}; this build reads {}) and was left as it is",
            self.path, self.version, self.supported
        )
    }
}

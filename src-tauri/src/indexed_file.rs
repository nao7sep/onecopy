//! Read-only resolution of a logical item to its current deterministic file.
//!
//! Webviews identify indexed content by hash or path id; filesystem paths
//! never cross into commands as authority. Hash resolution follows the same
//! date-then-path ordering as item presentation, so filename, attributes,
//! text, and external delegation all describe one representative copy.

use std::path::PathBuf;

use rusqlite::Connection;

pub fn live_path(
    conn: &Connection,
    hash: Option<&str>,
    path_id: Option<i64>,
) -> Result<PathBuf, String> {
    let path: String = match (hash.filter(|value| !value.is_empty()), path_id) {
        (Some(hash), None) => conn
            .query_row(
                "SELECT abs_path FROM paths \
                 WHERE content_hash = ?1 AND missing = 0 \
                 ORDER BY resolved_utc_ms IS NULL, resolved_utc_ms, \
                          abs_path COLLATE onecopy_nocase, abs_path \
                 LIMIT 1",
                [hash],
                |row| row.get(0),
            )
            .map_err(|_| "no live copy of this item".to_string())?,
        (None, Some(path_id)) => conn
            .query_row(
                "SELECT abs_path FROM paths WHERE id = ?1 AND missing = 0",
                [path_id],
                |row| row.get(0),
            )
            .map_err(|_| "no live copy of this item".to_string())?,
        _ => return Err("item needs exactly one hash or pathId".to_string()),
    };
    Ok(PathBuf::from(path))
}

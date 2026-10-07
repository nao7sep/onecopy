//! Read-only resolution of a logical item to its current deterministic file.
//!
//! Webviews identify indexed content by hash or path id; filesystem paths
//! never cross into commands as authority. Hash resolution follows the same
//! canonical presentation order, choosing the first reachable copy.

use std::path::PathBuf;

use rusqlite::Connection;

/// The one spelling of a logical item's key across the boundary: its content
/// hash, or `path-<id>` for an item without one. Webviews match media-use
/// release keys against their players' keys in this spelling.
pub fn item_key(hash: Option<&str>, path_id: i64) -> String {
    hash.map_or_else(|| format!("{PATH_KEY_PREFIX}{path_id}"), str::to_owned)
}

/// The item a key names: `(hash, path id)`, exactly one of them set; `None`
/// when a `path-` key does not carry a number.
pub fn parse_item_key(key: &str) -> Option<(Option<&str>, Option<i64>)> {
    match key.strip_prefix(PATH_KEY_PREFIX) {
        Some(id) => id.parse().ok().map(|id| (None, Some(id))),
        None => Some((Some(key), None)),
    }
}

const PATH_KEY_PREFIX: &str = "path-";

pub fn live_path(
    conn: &Connection,
    hash: Option<&str>,
    path_id: Option<i64>,
) -> Result<PathBuf, String> {
    live_path_with(conn, hash, path_id, &|path| {
        crate::file_identity::open_regular_nofollow(path).is_ok()
    })
}

/// Resolve in presentation order, with an injectable bounded reachability probe.
pub fn live_path_with(
    conn: &Connection,
    hash: Option<&str>,
    path_id: Option<i64>,
    reachable: &dyn Fn(&std::path::Path) -> bool,
) -> Result<PathBuf, String> {
    available_path_with(conn, hash, path_id, reachable)?
        .ok_or_else(|| "no available copy of this item".to_string())
}

/// Background work can leave unavailable content pending without turning it
/// into a failed decode. Database failures remain errors to the work owner.
pub fn available_path(conn: &Connection, hash: Option<&str>, path_id: Option<i64>) -> Result<Option<PathBuf>, String> {
    available_path_with(conn, hash, path_id, &|path| crate::file_identity::open_regular_nofollow(path).is_ok())
}

fn available_path_with(conn: &Connection, hash: Option<&str>, path_id: Option<i64>, reachable: &dyn Fn(&std::path::Path) -> bool) -> Result<Option<PathBuf>, String> {
    let paths: Vec<String> = match (hash.filter(|value| !value.is_empty()), path_id) {
        (Some(hash), None) => {
            let mut statement = conn.prepare(
                "SELECT p.abs_path FROM paths p JOIN logical_contents l ON l.content_hash = p.content_hash
                 WHERE p.content_hash = ?1 AND p.missing = 0 AND p.companion_of IS NULL
                 ORDER BY (p.id = l.representative_path_id) DESC, p.review_visible DESC,
                          p.resolved_utc_ms IS NULL, p.resolved_utc_ms,
                          p.abs_path COLLATE onecopy_nocase, p.abs_path"
            ).map_err(|error| error.to_string())?;
            let rows = statement.query_map([hash], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            rows.collect::<rusqlite::Result<_>>().map_err(|error| error.to_string())?
        }
        (None, Some(path_id)) => {
            let mut statement = conn.prepare("SELECT abs_path FROM paths WHERE id = ?1 AND missing = 0")
                .map_err(|error| error.to_string())?;
            let rows = statement.query_map([path_id], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            rows.collect::<rusqlite::Result<_>>().map_err(|error| error.to_string())?
        }
        _ => return Err("item needs exactly one hash or pathId".to_string()),
    };
    Ok(paths.into_iter().map(PathBuf::from).find(|path| reachable(path)))
}

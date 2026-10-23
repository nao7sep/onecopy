//! The single source of truth for onecopy's storage root.
//!
//! Per the storage-path conventions, the Tauri **Rust core** is the only path
//! resolver — the sandboxed webview never computes a data path. The root is
//! `ONECOPY_DATA_DIR` when that variable is set and non-empty; otherwise it
//! defaults to `~/.onecopy`. The override value is expanded (a leading `~`
//! becomes the home directory) and made absolute against the **home**
//! directory — never the current working directory — so the location the app
//! reads and writes can never depend on how the process was launched.
//!
//! Startup resolves the root once through `resolve_data_root` and settles it;
//! every later reader (commands, workers, protocols) reads the settled root
//! through `data_root`, so there is one source of truth and the frontend
//! never reconstructs `~/.onecopy` itself.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

const DATA_DIR_NAME: &str = ".onecopy";
const DATA_DIR_ENV_VAR: &str = "ONECOPY_DATA_DIR";

// The standard subdirectory/file names under the root, owned here so one
// module names every standard subpath (storage-path conventions; storage.rs
// names the JSON stores and the backup store defines its own DB name, all
// pinned together by the storage_file_names integration test).
pub const LOGS_DIR_NAME: &str = "logs";
pub const BIN_DIR_NAME: &str = "bin";
pub const MODELS_DIR_NAME: &str = "models";
pub const TEMP_DIR_NAME: &str = "temp";
pub const DEPENDENCIES_FILE_NAME: &str = "dependencies.json";
pub const SOURCE_VOLUMES_FILE_NAME: &str = "source-volumes.json";

static SETTLED_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The storage root startup settled. Every reader after startup uses this;
/// before startup has prepared the root (or when it was blocked) there is
/// none, and nothing may read or write app data.
pub fn data_root() -> Result<PathBuf, String> {
    SETTLED_ROOT
        .get()
        .cloned()
        .ok_or_else(|| "data root unset".to_string())
}

/// The derived-media cache under the settled root.
pub fn cache_root() -> Result<PathBuf, String> {
    Ok(data_root()?.join(crate::storage::CACHE_DIR_NAME))
}

/// Records the root startup resolved and prepared; set exactly once.
pub(crate) fn settle_data_root(root: PathBuf) -> Result<(), String> {
    SETTLED_ROOT
        .set(root)
        .map_err(|_| "data root was initialized more than once".to_string())
}

// Resolves the absolute storage root and ensures it exists — for the launch
// owners that run before the root is settled (process ownership, startup).
// Returns a clear error (and the caller stops) if the home directory is
// unknown or the root cannot be created — never a silent fallback to a
// different location.
pub fn resolve_data_root(app: &AppHandle) -> Result<PathBuf, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| format!("could not resolve home directory: {e}"))?;
    let root = resolve_root(&home, std::env::var(DATA_DIR_ENV_VAR).ok())?;
    create_data_root(&root)
        .map_err(|e| format!("could not create storage root {}: {e}", crate::native_path::label(&root)))?;
    #[cfg(unix)]
    ensure_private(&root);
    Ok(root)
}

// Creates the root (and any missing parents) owner-only from the start on
// POSIX, rather than creating it under the default umask and relying solely
// on `ensure_private` to tighten it afterward — that sequence leaves a window
// where a freshly created root is briefly world-readable. `ensure_private`
// still runs after this to tighten a root an earlier build left
// broader than 0700; this only narrows the mode a *new* root is born with.
#[cfg(unix)]
fn create_data_root(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
}

#[cfg(windows)]
fn create_data_root(root: &Path) -> std::io::Result<()> {
    crate::data_root_privacy::prepare(root)
}

/// Restricts the data root to the owner (0700) so previews, transcripts, the
/// index and logs never inherit broader access than the source material they
/// are derived from: on macOS the home directory is `drwxr-x---`
/// (group `staff`, every standard account's primary group), while this app's
/// default `create_dir_all` umask left `~/.onecopy` world-readable. A
/// directory this restrictive blocks traversal into anything beneath it
/// regardless of that entry's own mode, so nothing further needs tightening.
/// Runs every time the resolver runs, exactly like the `create_dir_all`
/// beside it — cheap, and it also tightens a root an earlier build left too
/// open, without a separate migration step. A failure
/// is a logged warning, not a fatal error: the data root itself is still
/// usable.
#[cfg(unix)]
fn ensure_private(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(error) = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)) {
        crate::logging::warn(
            "could not restrict the data root to the owner",
            serde_json::json!({
                "path": crate::native_path::encode(root),
                "error": { "message": error.to_string() },
            }),
        );
    }
}

// The storage root as the app will resolve it, found before Tauri builds the
// app (the interface language must be read before then; see i18n::align_appkit).
// It uses the same home directory Tauri's path resolver returns and creates
// nothing; None if the home or the override cannot be resolved.
pub fn data_root_before_launch() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    resolve_root(&home, std::env::var(DATA_DIR_ENV_VAR).ok()).ok()
}

/// Whether `candidate` is the app's own data root, or lies beneath it — the
/// app's index, logs, caches and models, which are never source content to
/// walk, watch or offer as a destination. This mirrors
/// `trash::is_trash_path`: a plain path-component comparison, cheap enough to
/// run on every walked and watched entry, not a filesystem identity check.
/// `data_root` is the app's already-resolved storage root; a source that
/// reaches the same storage only through a symlink or another spelling is not
/// caught here, the same tradeoff the trash exclusion already accepts.
pub fn is_within_data_root(candidate: &Path, data_root: &Path) -> bool {
    crate::path_identity::lexical_is_within(candidate, data_root)
}

// Root resolution, factored out so it can be unit-tested with an injected home
// directory. `override_value` is the raw `ONECOPY_DATA_DIR` value (if any). The
// value is expanded (environment references first, then a leading `~`) and made
// absolute against the home directory. An override that is set but expands to
// nothing — an unset `$VAR`/`%VAR%`, say — is a reported error, never a silent
// collapse onto the bare home directory.
fn resolve_root(home: &Path, override_value: Option<String>) -> Result<PathBuf, String> {
    let Some(raw) = override_value else {
        return Ok(home.join(DATA_DIR_NAME));
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(home.join(DATA_DIR_NAME));
    }
    let expanded = expand_env_references(trimmed);
    let expanded = expanded.trim();
    if expanded.is_empty() {
        return Err(format!(
            "{DATA_DIR_ENV_VAR} is set to \"{raw}\" but expands to an empty path \
             (an unset $VAR/%VAR%?). Set it to a usable directory, or unset it to use ~/{DATA_DIR_NAME}."
        ));
    }
    Ok(absolutize(home, expand_tilde(home, expanded)))
}

// Expands a leading `~` / `~/` (and `~\` on Windows) to the home directory.
fn expand_tilde(home: &Path, value: &str) -> PathBuf {
    if value == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return home.join(rest);
    }
    PathBuf::from(value)
}

// Expands `${VAR}`, `$VAR` (POSIX) and `%VAR%` (Windows) references in the
// override against the environment. An unset reference expands to empty,
// matching shell behavior, rather than being left as a literal path segment.
// Identifier characters are ASCII, so all slicing lands on char boundaries.
fn expand_env_references(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("${") {
            if let Some(end) = after.find('}') {
                out.push_str(&std::env::var(&after[..end]).unwrap_or_default());
                rest = &after[end + 1..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('$') {
            let bytes = after.as_bytes();
            let mut n = 0;
            while n < bytes.len()
                && (bytes[n].is_ascii_alphanumeric() || bytes[n] == b'_')
                && !(n == 0 && bytes[n].is_ascii_digit())
            {
                n += 1;
            }
            if n > 0 {
                out.push_str(&std::env::var(&after[..n]).unwrap_or_default());
                rest = &after[n..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('%') {
            if let Some(end) = after.find('%') {
                let name = &after[..end];
                if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    out.push_str(&std::env::var(name).unwrap_or_default());
                    rest = &after[end + 1..];
                    continue;
                }
            }
        }
        let mut chars = rest.chars();
        let Some(next) = chars.next() else {
            break;
        };
        out.push(next);
        rest = chars.as_str();
    }
    out
}

// A relative override is resolved against the home directory (never the
// working directory), so the override can never reintroduce a cwd dependence.
fn absolutize(home: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        home.join(path)
    }
}

#[cfg(any(windows, test))]
pub(crate) const PRIVATE_ROOT_CERTIFICATE: &str = ".onecopy-private-root.json";

#[cfg(any(windows, test))]
/// Standard owned roots only. An unrelated file beside the app's stores is
/// never recursively visited or given a different DACL by recovery.
pub(crate) fn owned_data_entry(name: &std::ffi::OsStr) -> Option<bool> {
    let folded = name.to_str()?.to_ascii_lowercase();
    let name = folded.as_str();
    if [crate::storage::CACHE_DIR_NAME, LOGS_DIR_NAME, BIN_DIR_NAME,
        MODELS_DIR_NAME, TEMP_DIR_NAME].contains(&name) { return Some(true); }
    if [PRIVATE_ROOT_CERTIFICATE, "instance.lock", "instance.endpoint"].contains(&name) { return Some(false); }
    let stores = [crate::storage::CONFIG_FILE_NAME, crate::storage::STATE_FILE_NAME, crate::storage::WINDOW_FILE_NAME,
        crate::storage::PREVIEW_WINDOW_FILE_NAME, crate::storage::RECORDS_WINDOW_FILE_NAME, crate::storage::INDEX_DB_FILE_NAME,
        crate::records::RECORDS_DB_FILE_NAME, crate::backup_store::BACKUPS_DB_FILE_NAME,
        DEPENDENCIES_FILE_NAME, SOURCE_VOLUMES_FILE_NAME, crate::file_identity::INSTALLATION_ID_FILE_NAME];
    for store in stores {
        if name == store { return Some(false); }
        if let Some(suffix) = name.strip_prefix(store) {
            if store.ends_with(".sqlite3") && ["-wal", "-shm", "-journal"].contains(&suffix) { return Some(false); }
        }
        let stem = Path::new(store).file_stem()?.to_str()?;
        let Some(rest) = name.strip_prefix(stem).and_then(|rest| rest.strip_prefix('-')) else { continue; };
        if let Some(id) = rest.strip_suffix(".tmp") {
            if id.len() == 21 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') { return Some(false); }
        }
        let rest = rest.strip_suffix("-wal").or_else(|| rest.strip_suffix("-shm")).unwrap_or(rest);
        if let Some(stamp) = rest.strip_suffix(".invalid") {
            let bytes = stamp.as_bytes();
            if bytes.len() == 19 && bytes[8] == b'-' && &bytes[15..] == b"-utc"
                && bytes[..8].iter().chain(&bytes[9..15]).all(u8::is_ascii_digit) { return Some(false); }
        }
    }
    None
}


#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private
// `resolve_root`; promoting it would widen the crate's API only for this
// test.
#[path = "../tests/unit/paths.rs"]
mod tests;

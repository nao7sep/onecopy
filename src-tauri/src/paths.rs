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
        .map_err(|e| format!("could not create storage root {}: {e}", root.display()))?;
    #[cfg(unix)]
    ensure_private(&root);
    Ok(root)
}

// Creates the root (and any missing parents) owner-only from the start on
// POSIX, rather than creating it under the default umask and relying solely
// on `ensure_private` to tighten it afterward — that sequence leaves a window
// where a freshly created root is briefly world-readable. `ensure_private`
// still runs after this (R6-03) to tighten a root an earlier build left
// broader than 0700; this only narrows the mode a *new* root is born with.
#[cfg(unix)]
fn create_data_root(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
}

#[cfg(not(unix))]
fn create_data_root(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)
}

/// Restricts the data root to the owner (0700) so previews, transcripts, the
/// index and logs never inherit broader access than the source material they
/// are derived from (R6-03): on macOS the home directory is `drwxr-x---`
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
                "path": root,
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
/// walk, watch or offer as a destination (R6-02). This mirrors
/// `trash::is_trash_path`: a plain path-component comparison, cheap enough to
/// run on every walked and watched entry, not a filesystem identity check.
/// `data_root` is the app's already-resolved storage root; a source that
/// reaches the same storage only through a symlink or another spelling is not
/// caught here, the same tradeoff the trash exclusion already accepts.
pub fn is_within_data_root(candidate: &Path, data_root: &Path) -> bool {
    let mut candidate_components = candidate.components();
    for root_component in data_root.components() {
        match candidate_components.next() {
            Some(component) if component.as_os_str().eq_ignore_ascii_case(root_component.as_os_str()) => {}
            _ => return false,
        }
    }
    true
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

/// A data-root subfolder the webview may ask the OS to reveal. A vetted set,
/// not a join of caller input: the reveal command must never become "open any
/// path the webview asks for". Created on first reveal.
pub(crate) fn revealable_data_subdir(root: &Path, name: &str) -> Result<PathBuf, String> {
    let target = match name {
        "logs" => root.join(LOGS_DIR_NAME),
        other => return Err(format!("not a revealable folder: {other}")),
    };
    std::fs::create_dir_all(&target)
        .map_err(|error| format!("could not create {}: {error}", target.display()))?;
    Ok(target)
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private
// `resolve_root` and the crate-private `revealable_data_subdir`; promoting it would widen the crate's API only for this
// test.
#[path = "../tests/unit/paths.rs"]
mod tests;

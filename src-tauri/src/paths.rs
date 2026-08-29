//! The single source of truth for onecopy's storage root.
//!
//! Per the storage-path conventions, the Tauri **Rust core** is the only path
//! resolver — the sandboxed webview never computes a data path. The root is
//! `ONECOPY_HOME` when that variable is set and non-empty; otherwise it
//! defaults to `~/.onecopy`. The override value is expanded (a leading `~`
//! becomes the home directory) and made absolute against the **home**
//! directory — never the current working directory — so the location the app
//! reads and writes can never depend on how the process was launched.
//!
//! Both the log-file resolver (`run()` in `lib.rs`) and the `load_app_data`
//! command the frontend calls route through `data_root` here, so there is one
//! source of truth and the frontend never reconstructs `~/.onecopy` itself.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

const DATA_DIR_NAME: &str = ".onecopy";
const HOME_ENV_VAR: &str = "ONECOPY_HOME";

// The standard subdirectory/file names under the root, owned here so one
// module names every standard subpath (storage-path conventions; storage.rs
// names the JSON stores and the backup store defines its own DB name, all
// pinned together by the storage_file_names integration test).
pub const LOGS_DIR_NAME: &str = "logs";
pub const BIN_DIR_NAME: &str = "bin";
pub const MODELS_DIR_NAME: &str = "models";
pub const TEMP_DIR_NAME: &str = "temp";
pub const TRASH_DIR_NAME: &str = "trash";
pub const DEPENDENCIES_FILE_NAME: &str = "dependencies.json";
pub const SOURCE_VOLUMES_FILE_NAME: &str = "source-volumes.json";

// Resolves the absolute storage root and ensures it exists. Returns a clear
// error (and the caller stops) if the home directory is unknown or the root
// cannot be created — never a silent fallback to a different location.
pub fn data_root(app: &AppHandle) -> Result<PathBuf, String> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| format!("could not resolve home directory: {e}"))?;
    let root = resolve_root(&home, std::env::var(HOME_ENV_VAR).ok())?;
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("could not create storage root {}: {e}", root.display()))?;
    Ok(root)
}

// Root resolution, factored out so it can be unit-tested with an injected home
// directory. `override_value` is the raw `ONECOPY_HOME` value (if any). The
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
            "{HOME_ENV_VAR} is set to \"{raw}\" but expands to an empty path \
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
    if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
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
                if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_root_is_home_dot_onecopy() {
        let home = PathBuf::from("/home/tester");
        // Unset / empty / whitespace all fall back to the default root.
        assert_eq!(resolve_root(&home, None).unwrap(), home.join(".onecopy"));
        assert_eq!(resolve_root(&home, Some(String::new())).unwrap(), home.join(".onecopy"));
        assert_eq!(
            resolve_root(&home, Some("   ".to_string())).unwrap(),
            home.join(".onecopy")
        );
    }

    #[test]
    fn env_var_relocates_root_to_absolute_path() {
        let home = PathBuf::from("/home/tester");
        assert_eq!(
            resolve_root(&home, Some("/tmp/oc-test".to_string())).unwrap(),
            PathBuf::from("/tmp/oc-test")
        );
    }

    #[test]
    fn env_var_expands_leading_tilde() {
        let home = PathBuf::from("/home/tester");
        assert_eq!(resolve_root(&home, Some("~".to_string())).unwrap(), home);
        assert_eq!(
            resolve_root(&home, Some("~/profiles/work".to_string())).unwrap(),
            home.join("profiles/work")
        );
    }

    #[test]
    fn relative_env_var_resolves_against_home_not_cwd() {
        let home = PathBuf::from("/home/tester");
        assert_eq!(
            resolve_root(&home, Some("alt-root".to_string())).unwrap(),
            home.join("alt-root")
        );
    }

    #[test]
    fn expands_environment_references_in_the_override() {
        let home = PathBuf::from("/home/tester");
        std::env::set_var("ONECOPY_TEST_BASE", "/mnt/disk2");
        assert_eq!(
            resolve_root(&home, Some("$ONECOPY_TEST_BASE/oc".to_string())).unwrap(),
            PathBuf::from("/mnt/disk2/oc")
        );
        assert_eq!(
            resolve_root(&home, Some("${ONECOPY_TEST_BASE}/oc".to_string())).unwrap(),
            PathBuf::from("/mnt/disk2/oc")
        );
        std::env::remove_var("ONECOPY_TEST_BASE");
    }

    #[test]
    fn override_that_expands_to_empty_is_rejected() {
        let home = PathBuf::from("/home/tester");
        std::env::remove_var("ONECOPY_UNSET_FOR_TEST");
        assert!(resolve_root(&home, Some("$ONECOPY_UNSET_FOR_TEST".to_string())).is_err());
    }
}

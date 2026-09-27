//! Bounded best-effort filesystem recovery.
//!
//! Cleanup of private staging and reconstructible cache files must never hide
//! the primary failure or become an unbounded retry loop. It may continue when
//! cleanup fails, but the secondary failure still belongs in the session log.
//! Each step is a bounded `volume_io` call.

use std::path::Path;

use serde_json::json;

pub fn remove_file(path: &Path, context: &str) {
    match crate::volume_io::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => crate::logging::warn(
            "filesystem recovery failed",
            json!({
                "operation": "removeFile",
                "context": context,
                "path": path,
                "error": { "message": error.to_string() },
            }),
        ),
    }
}

pub fn create_dir_all(path: &Path, context: &str) -> bool {
    match crate::volume_io::create_dir_all(path) {
        Ok(()) => true,
        Err(error) => {
            crate::logging::warn(
                "filesystem recovery failed",
                json!({
                    "operation": "createDirectory",
                    "context": context,
                    "path": path,
                    "error": { "message": error.to_string() },
                }),
            );
            false
        }
    }
}

pub fn rename(from: &Path, to: &Path, context: &str) {
    if let Err(error) = crate::volume_io::rename(from, to) {
        crate::logging::warn(
            "filesystem recovery failed",
            json!({
                "operation": "rename",
                "context": context,
                "from": from,
                "to": to,
                "error": { "message": error.to_string() },
            }),
        );
    }
}

pub fn sync_all(file: &std::fs::File, path: &Path, context: &str) {
    if let Err(error) = file.sync_all() {
        crate::logging::warn(
            "filesystem recovery failed",
            json!({
                "operation": "sync",
                "context": context,
                "path": path,
                "error": { "message": error.to_string() },
            }),
        );
    }
}

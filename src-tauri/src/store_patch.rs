//! Config and state saves are PATCHES merged core-side: the core holds the
//! file, so it is the one owner of the read-modify-write, and no frontend
//! store's stale cached copy can blind-overwrite another's save. A config
//! patch is validated first and then brought into the running app (theme,
//! native menu language, appearance invalidation, the watcher). Each returns
//! the merged document — the config as its effective values — so the caller
//! can publish it without a second read.

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::{
    ai_acceleration, failure_runtime, i18n, logging, menu, paths, resolution, scan_runtime,
    storage, theme, visibility, watcher,
};

pub fn patch_config(app: &AppHandle, mut patch: Value, report_failure: bool) -> Result<Value, String> {
    let result = logging::boundary(
        "patch_config",
        json!({}),
        || {
            let previous_source_dirs = if patch.get("sourceDirs").is_some() {
                let data_root = paths::data_root()?;
                Some(storage::load_config_source_dirs(&data_root)?)
            } else {
                None
            };
            if let Some(value) = patch.get_mut("defaultTimezone") {
                let name = value
                    .as_str()
                    .ok_or("Default timezone must be an IANA timezone name")?;
                *value = Value::String(resolution::parse_timezone_name(name)?.to_string());
            }
            ai_acceleration::validate_patch(&patch)?;
            visibility::Policy::from_config(&patch)?;
            let outcome = storage::patch_config(&patch)?;
            report_quarantine(app, outcome.quarantined);
            // The theme is applied natively to every window as part of the
            // save; pages follow it through prefers-color-scheme.
            if patch.get("theme").is_some() {
                if let Err(error) =
                    theme::apply_everywhere(app, theme::config_window_theme(&outcome.merged))
                {
                    logging::warn(
                        "saved theme could not be applied to every window",
                        json!({ "error": { "message": error } }),
                    );
                }
            }
            // A saved language reaches the native menu here; the windows follow
            // through the appearance invalidation below. The items macOS draws
            // itself keep the language AppKit settled on at launch.
            if patch.get("language").is_some() {
                let state = app.state::<i18n::LanguageState>();
                let resolved = i18n::normalize_preference(
                    outcome.merged.get("language").and_then(Value::as_str),
                )
                .unwrap_or(state.system_language);
                state.set_current(resolved);
                if let Err(error) = menu::build(app, resolved).and_then(|menu| app.set_menu(menu)) {
                    logging::warn(
                        "saved language could not be applied to the native menu",
                        json!({ "error": { "message": error.to_string() } }),
                    );
                }
            }
            // Invalidation, not a potentially stale snapshot from a racing save.
            failure_runtime::emit_or_record(app, "appearance://changed", json!({}));
            let current_source_dirs = outcome
                .merged
                .get("sourceDirs")
                .and_then(Value::as_array)
                .map(|dirs| {
                    dirs.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if previous_source_dirs
                .as_ref()
                .is_some_and(|previous| previous != &current_source_dirs)
            {
                if let Err(error) = watcher::start(app.clone(), current_source_dirs) {
                    scan_runtime::record_runtime_failure(app, "watcher-failed", &error);
                }
            }
            Ok(storage::effective_config(Some(&outcome.merged)))
        },
        |_| json!({}),
    );
    if report_failure {
        if let Err(error) = &result {
            let _ = failure_runtime::report(app, "config-save-failed", None, error);
        }
    }
    result
}

pub fn patch_state(app: &AppHandle, patch: &Value, report_failure: bool) -> Result<Value, String> {
    let result = logging::boundary(
        "patch_state",
        json!({}),
        || {
            let outcome = storage::patch_state(patch)?;
            report_quarantine(app, outcome.quarantined);
            Ok(outcome.merged)
        },
        |_| json!({}),
    );
    if report_failure {
        if let Err(error) = &result {
            let _ = failure_runtime::report(app, "state-save-failed", None, error);
        }
    }
    result
}

/// A store can also be quarantined mid-session — a patch reads the file it is
/// about to merge into — where there is no load result to ride home on. The
/// patch hands its own outcome here, and it is pushed to the same reporting
/// surface, so the rule ("every quarantine reaches the user") has no hole.
fn report_quarantine(app: &AppHandle, record: Option<storage::QuarantineRecord>) {
    if let Some(record) = record {
        failure_runtime::emit_or_record(
            app,
            "storage://quarantined",
            json!({ "quarantines": [record] }),
        );
    }
}

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

pub mod ai_acceleration;
#[cfg(feature = "ai-test-engine")]
pub mod ai_test_engine;
#[cfg(feature = "app-e2e")]
pub mod ai_test_instrumentation;
pub mod background_work;
pub mod backup_store;
pub mod binaries;
mod binaries_acquisition;
pub mod binaries_manager;
pub mod derived_runtime;
pub mod derived_state;
pub mod derived_work;
pub mod extensions;
pub mod face;
pub mod failure_runtime;
pub mod file_identity;
pub mod file_information_runtime;
pub mod fs_publish;
pub mod fs_recovery;
pub mod hashing;
pub mod index_store;
pub mod indexed_file;
mod instance_owner;
pub mod issue_recovery;
pub mod live_photo;
pub mod logging;
pub mod media_protocol;
pub mod media_use;
pub mod metadata;
mod mutation_runtime;
mod nanoid;
pub mod notifications;
pub mod operations;
pub mod path_identity;
pub mod paths;
pub mod preview;
pub mod queries;
pub mod resolution;
pub mod resource_limits;
pub mod scan_runtime;
pub mod scanner;
pub mod similar_exclusions;
pub mod similarity;
pub mod source_check_runtime;
pub mod storage;
pub mod subprocess;
pub mod text_preview;
pub mod timestamps;
pub mod transcription;
pub mod trash;
pub mod video;
pub mod viewer_sequence;
pub mod volume;
pub mod watcher;
pub mod winpath;

// Records the panic payload, location, and (when RUST_BACKTRACE is set) the
// backtrace, flushes, then defers to the previous hook so the process still
// aborts and prints as usual.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let backtrace = std::backtrace::Backtrace::capture();
        logging::error(
            "panic",
            json!({
                "error": {
                    "message": payload,
                    "location": location,
                    "backtrace": format!("{backtrace}"),
                }
            }),
        );
        // The error line is already on disk (the logger is unbuffered); defer to
        // the previous hook so the process still aborts and prints as usual.
        default_hook(info);
    }));
}

// --- Commands ---
//
// Every fallible command runs inside logging::boundary(): one `debug` line at
// the start, one `info` (success) or `error` (failure) line with the elapsed
// duration. Expected outcomes are modeled as serde-tagged enums where they
// arise, not as errors.

// Config + state + data root in one startup round-trip.
// THREADING: Tauri dispatches a plain `#[tauri::command]` on the MAIN thread
// (`ExecutionContext::Blocking` in tauri-macros). On macOS the main thread
// also commits the window's layer updates, so a command doing file, network,
// subprocess or decode work freezes the visible UI for its whole duration —
// which is how a 40 ms check came to feel like half a second of dead button.
// Every command below that touches the disk, the network, a subprocess or the
// index carries `(async)` so it runs on the async runtime instead. The ones
// left plain are pure or atomic (validate_timezone, logging_debug_enabled,
// transcribe_cancel), or must keep strict call order (log_event). The get_*
// reads are `(async)` as well — their responses may now arrive OUT OF ORDER,
// which the stores absorb with request-sequence guards (`staleGuard` in
// src/state/request-seq.ts): a 30k-item month query on the main thread was
// exactly the block a slow machine felt as a frozen window.
#[tauri::command(async)]
fn load_app_data(app: AppHandle) -> Result<storage::LoadedAppData, String> {
    logging::boundary(
        "load_app_data",
        json!({}),
        || {
            let mut data = storage::load_app_data(&app)?;
            data.debug_enabled = logging::debug_enabled();
            Ok(data)
        },
        |d| {
            json!({
                "hasConfig": d.config.is_some(),
                "hasState": d.state.is_some(),
                "quarantines": d.quarantines.len(),
            })
        },
    )
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

#[tauri::command]
fn record_interface_failure(window: tauri::WebviewWindow, message: String) -> Result<(), String> {
    failure_runtime::report(
        window.app_handle(),
        "interface-failed",
        Some(window.label()),
        &message,
    )
}

// Config and state saves are PATCHES merged core-side: the core holds the
// file, so it is the one owner of the read-modify-write, and no frontend
// store's stale cached copy can blind-overwrite another's save. Returns the
// merged document so the caller can publish it without a second read.
#[tauri::command(async)]
fn patch_config(
    app: AppHandle,
    mut patch: Value,
    report_failure: Option<bool>,
) -> Result<Value, String> {
    let result = logging::boundary(
        "patch_config",
        json!({}),
        || {
            let previous_source_dirs = if patch.get("sourceDirs").is_some() {
                let data_root = paths::data_root(&app)?;
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
            let outcome = storage::patch_config(&app, &patch)?;
            report_quarantine(&app, outcome.quarantined);
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
                watcher::start(app.clone(), current_source_dirs);
            }
            Ok(outcome.merged)
        },
        |_| json!({}),
    );
    if report_failure.unwrap_or(true) {
        if let Err(error) = &result {
            let _ = failure_runtime::report(&app, "config-save-failed", None, error);
        }
    }
    result
}

#[tauri::command(async)]
fn patch_state(app: AppHandle, patch: Value) -> Result<Value, String> {
    let result = logging::boundary(
        "patch_state",
        json!({}),
        || {
            let outcome = storage::patch_state(&app, &patch)?;
            report_quarantine(&app, outcome.quarantined);
            Ok(outcome.merged)
        },
        |_| json!({}),
    );
    if let Err(error) = &result {
        let _ = failure_runtime::report(&app, "state-save-failed", None, error);
    }
    result
}

// The storage root, for the mediafile protocol's hash→path lookups.
pub(crate) static DATA_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
static EXIT_QUIESCING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static EXIT_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn cache_root() -> Option<std::path::PathBuf> {
    DATA_ROOT
        .get()
        .map(|root| root.join(storage::CACHE_DIR_NAME))
}

#[tauri::command(async)]
fn start_source_check(app: AppHandle) -> Result<bool, String> {
    source_check_runtime::start(app.clone()).map_err(|error| {
        let _ = failure_runtime::report(&app, "source-check-failed", None, &error);
        error
    })
}

#[tauri::command(async)]
fn stop_source_check(app: AppHandle) -> bool {
    source_check_runtime::stop(&app)
}

#[tauri::command]
fn set_file_information_paused(app: AppHandle, paused: bool) {
    file_information_runtime::set_paused(app, paused);
}

#[tauri::command]
fn admit_background_completion(app: AppHandle) {
    file_information_runtime::wake(app);
    derived_work::admit_automatic();
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexWorkSnapshot {
    source_check: source_check_runtime::Snapshot,
    file_information: file_information_runtime::Snapshot,
}

#[tauri::command(async)]
fn index_work_snapshot(app: AppHandle) -> Result<IndexWorkSnapshot, String> {
    let data_root = paths::data_root(&app)?;
    Ok(IndexWorkSnapshot {
        source_check: source_check_runtime::snapshot(),
        file_information: file_information_runtime::snapshot(&data_root),
    })
}

#[tauri::command(async)]
fn rebuild_library_index(app: AppHandle) -> Result<(), String> {
    logging::boundary(
        "rebuild_library_index",
        json!({}),
        || {
            // The mutation claim makes the contract race-free: an active file
            // operation rejects this command, and no new one can begin while
            // the reconstructible database facts are being cleared.
            let _rebuild = mutation_runtime::begin_rebuild(&app)?;
            let _media = media_use::begin(&app, &[])?;
            scan_runtime::run_foreground(&app, || {
                let data_root = paths::data_root(&app)?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                index_store::clear_reconstructible(&conn)?;
                notifications::clear_active(&app)
            })?;
            let _ = source_check_runtime::start(app.clone())?;
            Ok(())
        },
        |_| json!({}),
    )
}

#[derive(serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct SourceDirsStatus {
    missing: Vec<String>,
    substituted: Vec<String>,
}

// Presence AND identity verification over the configured source dirs: a dir
// that is not there is missing; a dir whose volume identity differs from the
// recorded one is substituted (the developer's backup drives share identical
// trees, so presence alone proves nothing). First sight records the identity
// — the "when the directory was added" moment as the core observes it. Rows
// for since-removed dirs are pruned; a volume without a readable identity
// degrades to presence-only, logged at debug.
fn verify_source_dirs(app: &AppHandle) -> Result<SourceDirsStatus, String> {
    let data_root = paths::data_root(app)?;
    let config = storage::read_config_for_setup(&data_root)?;
    let settings = scanner::settings_from_config(config.as_ref(), &data_root, 0);
    let mut status = SourceDirsStatus::default();
    for dir in &settings.source_dirs {
        let path = std::path::Path::new(dir);
        if !path.is_dir() {
            status.missing.push(dir.clone());
            continue;
        }
        let Some(current) = volume::volume_identity(path) else {
            logging::debug(
                "no volume identity readable; presence-only verification",
                json!({ "dir": dir }),
            );
            continue;
        };
        match volume::check_identity(&data_root, dir, &current)? {
            volume::IdentityCheck::FirstSight => logging::info(
                "source volume identity recorded",
                json!({ "dir": dir, "identity": current }),
            ),
            volume::IdentityCheck::Substituted { recorded } => {
                logging::warn(
                    "source volume SUBSTITUTED",
                    json!({ "dir": dir, "recorded": recorded, "current": current }),
                );
                status.substituted.push(dir.clone());
            }
            volume::IdentityCheck::Unchanged => {}
        }
    }

    // Identities for directories no longer configured are stale — prune.
    volume::prune_identities(&data_root, &settings.source_dirs)?;

    Ok(status)
}

// Deletes an ordered logical-item set under one mutation/media boundary. The
// core plans every physical copy and companion before changing the first file;
// progress and cancellation belong to the shared ephemeral mutation runtime.
#[tauri::command(async)]
fn delete_items(
    app: AppHandle,
    items: Vec<operations::ItemIdentity>,
    permanent: bool,
) -> Result<operations::DeleteBatchOutcome, String> {
    mutation_runtime::delete_items(&app, items, permanent)
}

#[tauri::command(async)]
fn mutation_cancel(app: AppHandle, operation_id: u64) -> Result<bool, String> {
    mutation_runtime::request_cancel(operation_id).map_err(|error| {
        failure_runtime::report(&app, "file-operation-state-failed", None, &error)
            .err()
            .unwrap_or(error)
    })
}

fn item_projection_context(
    data_root: &std::path::Path,
) -> Result<queries::ItemProjectionContext, String> {
    Ok(queries::ItemProjectionContext {
        capabilities: derived_work::work_capabilities(data_root)?,
    })
}

#[tauri::command(async)]
fn get_section_window(
    app: AppHandle,
    kind: String,
    month: String,
    sort: queries::SectionSort,
    start: u64,
    limit: u32,
) -> Result<queries::SectionWindow, String> {
    logging::boundary(
        "get_section_window",
        json!({ "kind": kind, "month": month, "start": start, "limit": limit }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::section_window(
                &conn,
                &kind,
                &month,
                display_timezone(),
                sort,
                start,
                limit,
                item_projection_context(&data_root)?,
            )
        },
        |window| json!({ "total": window.total, "start": window.start, "items": window.items.len() }),
    )
}

#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
fn reconcile_section(
    app: AppHandle,
    kind: String,
    month: String,
    sort: queries::SectionSort,
    selected: Vec<queries::SectionIdentity>,
    anchor: Option<queries::SectionIdentity>,
    range_origin: Option<queries::SectionIdentity>,
    range_base: Vec<queries::SectionIdentity>,
    recovery: Option<queries::SectionRecoveryContext>,
    select_first: bool,
    limit: u32,
) -> Result<queries::SectionReconciliation, String> {
    logging::boundary(
        "reconcile_section",
        json!({
            "kind": kind,
            "month": month,
            "selected": selected.len(),
            "rangeBase": range_base.len(),
            "selectFirst": select_first,
            "limit": limit,
        }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::reconcile_section(
                &conn,
                &kind,
                &month,
                display_timezone(),
                sort,
                &selected,
                anchor.as_ref(),
                range_origin.as_ref(),
                &range_base,
                recovery.as_ref(),
                select_first,
                limit,
                item_projection_context(&data_root)?,
            )
        },
        |result| {
            json!({
                "total": result.window.total,
                "start": result.window.start,
                "items": result.window.items.len(),
                "selected": result.selected.len(),
            })
        },
    )
}

#[tauri::command(async)]
fn get_section_range(
    app: AppHandle,
    kind: String,
    month: String,
    sort: queries::SectionSort,
    start: u64,
    end: u64,
) -> Result<Vec<queries::PositionedSectionIdentity>, String> {
    logging::boundary(
        "get_section_range",
        json!({ "kind": kind, "month": month, "start": start, "end": end }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::section_range(
                &conn,
                &kind,
                &month,
                display_timezone(),
                sort,
                start,
                end,
            )
        },
        |items| json!({ "items": items.len() }),
    )
}

#[tauri::command(async)]
fn get_section_family_context(
    app: AppHandle,
    kind: String,
    month: String,
    sort: queries::SectionSort,
    member_hashes: Vec<String>,
) -> Result<Option<queries::SectionRecoveryContextOutput>, String> {
    logging::boundary(
        "get_section_family_context",
        json!({ "kind": kind, "month": month, "members": member_hashes.len() }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::section_family_context(
                &conn,
                &kind,
                &month,
                display_timezone(),
                sort,
                &member_hashes,
            )
        },
        |context| json!({ "found": context.is_some() }),
    )
}

#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
fn viewer_sequence_start(
    app: AppHandle,
    kind: String,
    month: String,
    sort: queries::SectionSort,
    selected: Vec<queries::PositionedSectionIdentity>,
    anchor: queries::SectionIdentity,
) -> Result<viewer_sequence::Snapshot, String> {
    logging::boundary(
        "viewer_sequence_start",
        json!({ "kind": kind, "month": month, "selected": selected.len() }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            viewer_sequence::start(
                &data_root,
                &conn,
                &kind,
                &month,
                display_timezone(),
                sort,
                selected,
                &anchor,
                item_projection_context(&data_root)?,
            )
        },
        |snapshot| json!({ "length": snapshot.length, "index": snapshot.index }),
    )
}

#[tauri::command(async)]
fn viewer_sequence_move(
    app: AppHandle,
    token: String,
    movement: viewer_sequence::Move,
) -> Result<viewer_sequence::Snapshot, String> {
    let data_root = paths::data_root(&app)?;
    let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
    viewer_sequence::move_current(
        &token,
        movement,
        &conn,
        item_projection_context(&data_root)?,
    )
}

#[tauri::command(async)]
fn viewer_sequence_reconcile(
    app: AppHandle,
    token: String,
) -> Result<Option<viewer_sequence::Snapshot>, String> {
    let data_root = paths::data_root(&app)?;
    let index_db = data_root.join(storage::INDEX_DB_FILE_NAME);
    let conn = index_store::open(&index_db)?;
    viewer_sequence::reconcile(
        &token,
        &index_db,
        &conn,
        item_projection_context(&data_root)?,
    )
}

#[tauri::command(async)]
fn viewer_sequence_close(token: Option<String>) -> Result<(), String> {
    viewer_sequence::close(token.as_deref())
}

#[tauri::command(async)]
fn comparison_selection_valid(app: AppHandle, hashes: Vec<String>) -> Result<bool, String> {
    let data_root = paths::data_root(&app)?;
    let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
    queries::comparison_selection_valid(&conn, &hashes)
}

fn display_timezone() -> chrono_tz::Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

// Moves or copies one ordered logical-item set to a destination directory. Modes:
// "move-trash-rest" (plain drag), "move-delete-rest" (Shift), "copy" (Cmd/Ctrl).
// Destinations under a configured source root are rejected — moving files into
// a scanned directory would only re-index them.
#[tauri::command(async)]
fn move_items_out(
    app: AppHandle,
    items: Vec<operations::ItemIdentity>,
    dest_dir: String,
    mode: String,
    conflict_policy: Option<String>,
    plan_token: Option<String>,
) -> Result<operations::MoveBatchOutcome, String> {
    mutation_runtime::move_items_out(
        &app,
        items,
        dest_dir,
        mode,
        conflict_policy,
        plan_token,
    )
}

// Destination-tree support: immediate subdirectories of one directory (the
// tree expands lazily; files are never listed — it is a destination panel, not
// a file manager).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DirEntry {
    name: String,
    path: String,
    has_children: bool,
    is_empty: bool,
}

#[tauri::command(async)]
fn list_subdirs(path: String) -> Result<Vec<DirEntry>, String> {
    list_subdirs_at(std::path::Path::new(&path))
}

fn list_subdirs_at(path: &std::path::Path) -> Result<Vec<DirEntry>, String> {
    let mut entries: Vec<DirEntry> = Vec::new();
    let read = std::fs::read_dir(path).map_err(|e| e.to_string())?;
    for entry in read {
        let entry = entry.map_err(|error| error.to_string())?;
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // dotfolders (incl. .onecopy-trash) stay out of the tree
        }
        let child_path = entry.path();
        let (has_children, is_empty) = child_directory_facts(&child_path)?;
        entries.push(DirEntry {
            name,
            path: child_path.to_string_lossy().to_string(),
            has_children,
            is_empty,
        });
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(entries)
}

fn child_directory_facts(path: &std::path::Path) -> Result<(bool, bool), String> {
    let children = std::fs::read_dir(path).map_err(|error| error.to_string())?;
    let mut is_empty = true;
    for child in children {
        let child = child.map_err(|error| error.to_string())?;
        is_empty = false;
        if child
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            return Ok((true, false));
        }
    }
    Ok((false, is_empty))
}

// EXCEPTION (tests-folder convention): destination listing is a private Tauri
// command, so its filesystem projection is pinned beside the helper it calls.
#[cfg(test)]
mod destination_listing_tests {
    use super::*;

    #[test]
    fn one_listing_projects_children_and_emptiness_together() {
        let root = tempfile::Builder::new()
            .prefix("onecopy-destinations-")
            .tempdir()
            .unwrap();
        std::fs::create_dir(root.path().join("empty")).unwrap();
        std::fs::create_dir(root.path().join("files-only")).unwrap();
        std::fs::write(root.path().join("files-only/item.txt"), b"item").unwrap();
        std::fs::create_dir_all(root.path().join("nested/child")).unwrap();
        std::fs::create_dir(root.path().join(".hidden")).unwrap();

        let rows = list_subdirs_at(root.path()).unwrap();
        let facts = |name: &str| {
            let row = rows.iter().find(|row| row.name == name).unwrap();
            (row.has_children, row.is_empty)
        };
        assert_eq!(facts("empty"), (false, true));
        assert_eq!(facts("files-only"), (false, false));
        assert_eq!(facts("nested"), (true, false));
        assert!(rows.iter().all(|row| row.name != ".hidden"));
    }
}

// Creates a subfolder under a tree node. The name must be case-insensitively
// unique within its directory (storage-path conventions' hard invariant).
#[tauri::command(async)]
fn create_subdir(parent: String, name: String) -> Result<String, String> {
    logging::boundary(
        "create_subdir",
        json!({ "parent": parent, "name": name }),
        || {
            let trimmed = name.trim();
            // Control characters (a pasted newline is the real case) would
            // create a directory whose name cannot be typed or read sanely.
            if trimmed.is_empty()
                || trimmed.contains(['/', '\\'])
                || trimmed.chars().any(char::is_control)
            {
                return Err(
                    "folder names must be non-empty, slash-free, and single-line".to_string(),
                );
            }
            let parent_path = std::path::Path::new(&parent);
            let lower = trimmed.to_lowercase();
            let read = std::fs::read_dir(parent_path).map_err(|error| error.to_string())?;
            for entry in read {
                let entry = entry.map_err(|error| error.to_string())?;
                if entry.file_name().to_string_lossy().to_lowercase() == lower {
                    return Err(format!(
                        "\"{trimmed}\" already exists here (names are case-insensitively unique)"
                    ));
                }
            }
            let target = parent_path.join(trimmed);
            std::fs::create_dir(&target).map_err(|e| e.to_string())?;
            Ok(target.to_string_lossy().to_string())
        },
        |path| json!({ "created": path }),
    )
}

// Deletes a tree folder ONLY when empty — remove_dir refuses otherwise, which
// is the entire safety model (empty folders render distinctly in the tree).
#[tauri::command(async)]
fn delete_empty_dir(path: String) -> Result<(), String> {
    logging::boundary(
        "delete_empty_dir",
        json!({ "path": path }),
        || std::fs::remove_dir(&path).map_err(|e| e.to_string()),
        |_| json!({}),
    )
}

// Opens a subdirectory of the app's data root in the OS file manager (the
// "Reveal logs folder" menu item). The path is BUILT HERE from paths.rs and a
// vetted subdir name — the frontend names a folder, never a path (paths.rs:
// "the frontend never reconstructs ~/.onecopy itself"). Routing through the
// opener plugin's RUST api rather than its JS command also sidesteps the
// plugin's permission scope, which applies to webview calls only — the JS
// route silently rejected every path because the scope allow-list was empty,
// and a `void openPath(...)` threw the rejection away.
#[tauri::command(async)]
fn reveal_data_subdir(app: AppHandle, name: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let root = paths::data_root(&app)?;
    // A vetted set, not a join of caller input: this command must never become
    // "open any path the webview asks for".
    let target = match name.as_str() {
        "logs" => root.join(paths::LOGS_DIR_NAME),
        other => return Err(format!("not a revealable folder: {other}")),
    };
    if !target.is_dir() {
        return Err(format!("{} does not exist yet", target.display()));
    }
    app.opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

// Opens an indexed item in its OS default app (the preview's "Open in player"
// codec-fallback). The path comes from the INDEX, never from the webview — a
// hash is resolved to a live copy here, so this can only ever open a file the
// scan actually indexed. Same reason as reveal_data_subdir for the Rust-side
// opener: the JS route was scope-rejected into a silent no-op, which left the
// fallback button for unplayable codecs doing nothing at all.
#[tauri::command(async)]
fn open_item_externally(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
) -> Result<(), String> {
    let result = logging::boundary(
        "open_item_externally",
        json!({ "hash": hash, "pathId": path_id }),
        || {
            use tauri_plugin_opener::OpenerExt;
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let path = indexed_file::live_path(&conn, hash.as_deref(), path_id)?;
            let key = hash
                .as_ref()
                .cloned()
                .or_else(|| path_id.map(|id| format!("path-{id}")))
                .ok_or_else(|| "item needs exactly one hash or pathId".to_string())?;
            let _media = media_use::begin_external(&app, &[key])?;
            app.opener()
                .open_path(path.to_string_lossy(), None::<&str>)
                .map_err(|error| error.to_string())
        },
        |_| json!({}),
    );
    if let Err(error) = &result {
        let _ = failure_runtime::report(&app, "external-open-failed", None, error);
    }
    result
}

#[tauri::command(async)]
fn text_preview(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
    encoding: Option<String>,
) -> Result<text_preview::PreviewBody, String> {
    let result = logging::boundary(
        "text_preview",
        json!({ "hash": hash, "pathId": path_id, "encoding": encoding }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let path = indexed_file::live_path(&conn, hash.as_deref(), path_id)?;
            let config = storage::read_config_for_setup(&data_root)?;
            let max_bytes = config
                .as_ref()
                .and_then(|value| value.get("textPreviewMaxBytes"))
                .and_then(Value::as_u64)
                .unwrap_or(text_preview::DEFAULT_MAX_BYTES)
                .max(1);
            let fallback = config
                .as_ref()
                .and_then(|value| value.get("textFallbackEncoding"))
                .and_then(Value::as_str)
                .unwrap_or(text_preview::DEFAULT_FALLBACK_ENCODING);
            text_preview::preview_file(&path, max_bytes, fallback, encoding.as_deref())
        },
        |body| match body {
            text_preview::PreviewBody::Text {
                byte_size,
                encoding,
                ..
            } => {
                json!({ "body": "text", "byteSize": byte_size, "encoding": encoding })
            }
            text_preview::PreviewBody::Attributes { byte_size, reason } => {
                json!({ "body": "attributes", "byteSize": byte_size, "reason": reason })
            }
            text_preview::PreviewBody::DecodeError {
                byte_size, reason, ..
            } => json!({ "body": "decodeError", "byteSize": byte_size, "reason": reason }),
        },
    );
    if let Err(error) = &result {
        let _ = failure_runtime::report(&app, "text-preview-failed", None, error);
    }
    result
}

#[tauri::command]
fn text_encodings() -> &'static [&'static str] {
    text_preview::encodings()
}

// Re-resolves every indexed item from stored evidence. Similarity is marked
// stale for its sole owner to rebuild; this command never performs derived
// work itself.
#[tauri::command(async)]
fn re_resolve_all(app: AppHandle) -> Result<u64, String> {
    logging::boundary(
        "re_resolve_all",
        json!({}),
        || {
            scan_runtime::run_foreground(&app, || {
                let data_root = paths::data_root(&app)?;
                let config = storage::read_config_for_setup(&data_root)?;
                let settings = scanner::settings_from_config(
                    config.as_ref(),
                    &data_root,
                    chrono::Utc::now().timestamp_millis(),
                );
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                // Resolution rows carry their own resumable debt. Pairing is an
                // atomic projection, so retain the existing coarse dirty-root
                // receipt across the whole Settings rebuild; cancellation before
                // publication must make a later index repair retry it.
                let repair_roots =
                    scanner::begin_scoped_index_repair(&conn, &settings.source_dirs)?;
                let stats = scanner::re_resolve_all_with_progress(
                    &conn,
                    &settings.resolution,
                    settings.pairing_enabled,
                    &|_| {},
                )?;
                scanner::complete_scoped_index_repair(&conn, &repair_roots)?;
                derived_work::wake();
                Ok(stats.resolved)
            })
        },
        |resolved| json!({ "resolved": resolved }),
    )
}

// Scoped rescan: re-stats exactly the directories that contributed files to
// one section (never the whole roots), then runs the pending pipeline tail.
// The full per-root walk remains the Scan button's escape hatch.
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum RescanSectionOutcome {
    Completed { changed: u64 },
    Cancelled,
}

#[tauri::command(async)]
fn rescan_section(
    app: AppHandle,
    kind: String,
    month: String,
) -> Result<RescanSectionOutcome, String> {
    logging::boundary(
        "rescan_section",
        json!({ "kind": kind, "month": month }),
        || {
            match scan_runtime::run_section(&app, || {
                let data_root = paths::data_root(&app)?;
                let config = storage::read_config_for_setup(&data_root)?;
                let settings = scanner::settings_from_config(
                    config.as_ref(),
                    &data_root,
                    chrono::Utc::now().timestamp_millis(),
                );
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                let dirs = queries::section_dirs(&conn, &kind, &month, display_timezone())?;
                let repair_roots = scanner::begin_scoped_index_repair(&conn, &dirs)?;
                let mut changed = 0u64;
                for dir in &dirs {
                    changed +=
                        watcher::restat_dir(&conn, std::path::Path::new(dir), &settings.lists)?;
                }
                // Finish any interrupted index checkpoints too. Derived media is
                // woken after the index tail instead of being smuggled into the
                // rescan command.
                let tail_owed = changed > 0 || scanner::pending_index_work_exists(&conn)?;
                if tail_owed {
                    let mut summary = scanner::ScanSummary::default();
                    scanner::run_index_tail_for_dirs(
                        &conn,
                        &settings,
                        &dirs,
                        &|_| {},
                        &mut summary,
                    )?;
                    derived_work::wake();
                }
                scanner::complete_scoped_index_repair(&conn, &repair_roots)?;
                Ok(changed)
            }) {
                Ok(changed) => Ok(RescanSectionOutcome::Completed { changed }),
                Err(error) if error == scanner::CANCELLED => Ok(RescanSectionOutcome::Cancelled),
                Err(error) => Err(error),
            }
        },
        |outcome| match outcome {
            RescanSectionOutcome::Completed { changed } => json!({ "status": "completed", "changed": changed }),
            RescanSectionOutcome::Cancelled => json!({ "status": "cancelled" }),
        },
    )
}

// The first-class issues surface: unreadable files, decode failures,
// copies-disagree anomalies, delete/copy errors — a silent skip never happens.
#[tauri::command(async)]
fn get_issues(app: AppHandle, limit: Option<u32>) -> Result<serde_json::Value, String> {
    logging::boundary(
        "get_issues",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let (total, rows) = queries::issues(&conn, limit.unwrap_or(500))?;
            Ok(json!({ "total": total, "rows": rows }))
        },
        |v| json!({ "total": v.get("total") }),
    )
}

#[tauri::command(async)]
fn get_recent_notifications(
    app: AppHandle,
    limit: Option<u32>,
) -> Result<serde_json::Value, String> {
    logging::boundary(
        "get_recent_notifications",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let (total, rows) = notifications::recent(&conn, limit.unwrap_or(500).min(500))?;
            Ok(json!({ "total": total, "rows": rows }))
        },
        |value| json!({ "total": value.get("total") }),
    )
}

#[tauri::command]
fn get_active_notifications() -> Vec<notifications::NotificationRecord> {
    notifications::active()
}

#[tauri::command(async)]
fn publish_notification(
    app: AppHandle,
    request: notifications::NotificationRequest,
) -> Result<notifications::NotificationRecord, String> {
    notifications::publish(&app, request)
}

#[tauri::command(async)]
fn record_recent_notification(
    app: AppHandle,
    request: notifications::NotificationRequest,
) -> Result<notifications::NotificationRecord, String> {
    let record = notifications::record_history(&app, request)?;
    failure_runtime::emit_or_record(&app, "notification://recorded", &record);
    Ok(record)
}

#[tauri::command(async)]
fn dismiss_notification(app: AppHandle, id: i64) -> Result<bool, String> {
    notifications::dismiss(&app, id)
}

// On-demand derive for ONE clicked photo the scan's bulk pass has not reached
// (walk-order; on a slow machine its tail is hours away). The preview surface
// calls this when its cache entry 404s, then reloads the entry. Idempotent
// and cheap when the entry already exists.
#[tauri::command(async)]
fn ensure_preview(app: AppHandle, hash: String) -> Result<String, String> {
    logging::boundary(
        "ensure_preview",
        json!({ "hash": hash }),
        || {
            let data_root = paths::data_root(&app)?;
            let config = storage::read_config_for_setup(&data_root)?;
            derived_work::ensure_preview(&app, &data_root, config.as_ref(), &hash)
        },
        |canonical| json!({ "canonicalHash": canonical }),
    )
}

// The 100% view's on-demand conversion for formats the webview cannot paint
// (HEIC/AVIF — WebView2 paints neither; routing every platform through the
// same path keeps behaviour identical and testable on macOS). Runs on the
// command pool, never in the synchronous protocol handler; the view calls
// this and then loads `mediacache://fullres-<hash>`.
#[tauri::command(async)]
fn ensure_fullres(app: AppHandle, hash: String) -> Result<(), String> {
    logging::boundary(
        "ensure_fullres",
        json!({ "hash": hash }),
        || {
            let _work = derived_runtime::begin_manual(&app, "previews")?;
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let cache_root = cache_root().ok_or("data root unset")?;
            let cache = preview::CachePaths::new(cache_root);
            // Presence decides availability, same rule as the scan settings.
            let ffmpeg = binaries_manager::ffmpeg_path(&data_root);
            let ffmpeg = ffmpeg.exists().then_some(ffmpeg);
            preview::ensure_fullres(&conn, &cache, ffmpeg.as_deref(), &hash)
        },
        |_| json!({}),
    )
}

// On-demand transcription (Design: Video handling): runs on its own thread —
// minutes-long and memory-heavy (~2–2.5 GB while running, released after) —
// with progress/done/error events; the transcript lands in the cache and
// `transcript_get` serves it thereafter. The process-wide claim prevents a
// manual run and coordinated background work from loading two models. Manual
// requests queue in submission order; cancellation applies to the active run.
#[tauri::command(async)]
fn transcribe(app: AppHandle, hash: String, replace: Option<bool>) -> Result<(), String> {
    let data_root = paths::data_root(&app)?;
    let cache_root = cache_root().ok_or("data root unset")?;
    let config = storage::read_config_for_setup(&data_root)?;
    let transcription_acceleration =
        ai_acceleration::selection_from_config(config.as_ref())?.transcription;
    let class = {
        let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
        let kind: String = conn
            .query_row(
                "SELECT kind FROM contents WHERE hash = ?1",
                [&hash],
                |row| row.get(0),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => "file is no longer available".to_string(),
                other => format!("could not read the file kind: {other}"),
            })?;
        match kind.as_str() {
            "video" => derived_state::WorkClass::VideoTranscripts,
            "audio" => derived_state::WorkClass::AudioTranscripts,
            _ => return Err("this file type cannot be transcribed".to_string()),
        }
    };
    let handle = app.clone();
    let start_hash = hash.clone();
    let started = std::thread::Builder::new()
        .name("onecopy-manual-transcription".to_string())
        .spawn(move || {
            let panic_handle = handle.clone();
            let panic_hash = hash.clone();
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let result = (|| -> Result<(String, String), String> {
                    let _work = derived_runtime::begin_manual_queued(&handle, class.id())?;
                    derived_runtime::active_item(&handle, class, &hash);
                    let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                    let projection = queries::ItemProjectionContext {
                        capabilities: derived_work::work_capabilities(&data_root)?,
                    };
                    let source_path: String = conn
                        .query_row(
                            "SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
                            [&hash],
                            |row| row.get(0),
                        )
                        .map_err(|error| match error {
                            rusqlite::Error::QueryReturnedNoRows => {
                                "no live copy of this file".to_string()
                            }
                            other => format!("could not read the file path: {other}"),
                        })?;
                    let cache = preview::CachePaths::new(cache_root);
                    let progress_handle = handle.clone();
                    let outcome = derived_work::complete_transcription_attempt(
                        derived_work::TranscriptionAttempt {
                            conn: &conn,
                            cache: &cache,
                            data_root: &data_root,
                            source_hash: &hash,
                            source_path: &source_path,
                            replace_existing: replace.unwrap_or(false),
                            acceleration: transcription_acceleration,
                            cancel_when: Some(Box::new(derived_runtime::cancelled)),
                        },
                        |exact_hash| {
                            if exact_hash != hash {
                                derived_work::notify_item_update(
                                    &handle,
                                    &conn,
                                    projection,
                                    class.id(),
                                    &hash,
                                    exact_hash,
                                );
                            }
                        },
                        |_| {},
                        move |progress_hash, percent| {
                            let percent = percent.clamp(0, 100);
                            derived_runtime::report_manual_progress(
                                &progress_handle,
                                class.id(),
                                percent as u64,
                                100,
                            );
                            failure_runtime::emit_or_record(
                                &progress_handle,
                                "transcribe://progress",
                                json!({ "hash": progress_hash, "percent": percent }),
                            );
                        },
                    )?;
                    match outcome {
                        derived_work::TranscriptionAttemptOutcome::Completed {
                            hash: exact_hash,
                            text,
                        } => {
                            derived_work::notify_item_update(
                                &handle,
                                &conn,
                                projection,
                                "transcripts",
                                &hash,
                                &exact_hash,
                            );
                            Ok((exact_hash, text))
                        }
                        derived_work::TranscriptionAttemptOutcome::Cancelled { .. } => {
                            // Preserve a typed cancellation after the claim resets its
                            // process-wide flag at the end of this worker.
                            Err(scanner::CANCELLED.to_string())
                        }
                        derived_work::TranscriptionAttemptOutcome::Unavailable {
                            hash: exact_hash,
                            message,
                        } => {
                            derived_work::notify_item_update(
                                &handle,
                                &conn,
                                projection,
                                "transcripts",
                                &hash,
                                &exact_hash,
                            );
                            Err(message)
                        }
                        derived_work::TranscriptionAttemptOutcome::ResourceSafety {
                            message,
                            ..
                        } => {
                            derived_work::pause_for_resource_safety(
                                &handle,
                                &conn,
                                class,
                                &message,
                            )?;
                            Err(message)
                        }
                        derived_work::TranscriptionAttemptOutcome::Failed {
                            hash: exact_hash,
                            message,
                        } => {
                            derived_work::notify_item_update(
                                &handle,
                                &conn,
                                projection,
                                "transcripts",
                                &hash,
                                &exact_hash,
                            );
                            Err(message)
                        }
                    }
                })();
                match result {
                    Ok((event_hash, text)) => failure_runtime::emit_checked(
                        &handle,
                        "transcribe://done",
                        json!({ "hash": event_hash, "text": text }),
                    ),
                    Err(err) if err == scanner::CANCELLED => failure_runtime::emit_checked(
                        &handle,
                        "transcribe://cancelled",
                        json!({ "hash": hash }),
                    ),
                    Err(err) => {
                        logging::warn(
                            "transcription failed",
                            json!({ "hash": hash, "error": { "message": err.clone() } }),
                        );
                        failure_runtime::emit_checked(
                            &handle,
                            "transcribe://error",
                            json!({ "hash": hash, "message": err }),
                        )
                    }
                }
            }));
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let _ = failure_runtime::report(
                        &panic_handle,
                        "event-delivery-failed",
                        Some(&panic_hash),
                        &error,
                    );
                }
                Err(payload) => {
                    let error = failure_runtime::panic_message(payload);
                    let _ = failure_runtime::report(
                        &panic_handle,
                        "transcription-worker-failed",
                        Some(&panic_hash),
                        &error,
                    );
                    if let Err(emit_error) = failure_runtime::emit_checked(
                        &panic_handle,
                        "transcribe://error",
                        json!({ "hash": panic_hash, "message": error }),
                    ) {
                        let _ = failure_runtime::report(
                            &panic_handle,
                            "event-delivery-failed",
                            Some("transcribe://error"),
                            &emit_error,
                        );
                    }
                }
            }
        });
    if let Err(error) = started {
        let message = format!("could not start transcription worker: {error}");
        let _ = failure_runtime::report(
            &app,
            "transcription-worker-failed",
            Some(&start_hash),
            &message,
        );
        return Err(message);
    }
    Ok(())
}

// The transcript's explicit output state. A missing cache entry behind a
// ready receipt is repaired back to pending here rather than displayed as a
// false success.
#[tauri::command(async)]
fn transcript_get(app: AppHandle, hash: String) -> Result<derived_state::TranscriptResult, String> {
    let data_root = paths::data_root(&app)?;
    let cache_root = cache_root().ok_or("data root unset")?;
    let cache = preview::CachePaths::new(cache_root);
    let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
    derived_state::transcript_result(&conn, &cache, &hash)
}

// Borderless presentation windows use macOS simple fullscreen so they cover
// system chrome without moving into a Space. Elsewhere their exact monitor
// bounds already cover the taskbar, so the command is deliberately a no-op.
// simple fullscreen (the pre-Lion kind) hides the menu bar and dock WITHOUT
// the Spaces animation that made real fullscreen unusable at keystroke pace.
#[tauri::command(async)]
fn set_window_simple_fullscreen(app: AppHandle, label: String, enable: bool) -> Result<(), String> {
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("no window labeled {label}"))?;
    #[cfg(target_os = "macos")]
    {
        window.set_simple_fullscreen(enable).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, enable);
        Ok(())
    }
}

// The frontend's throttled input ping — the coordinator's whole view
// of the user. Atomic store; keeping it plain (main-thread) is deliberate,
// it must never queue behind async work.
#[tauri::command]
fn note_user_activity() {
    derived_work::note_activity();
}

#[tauri::command]
fn media_use_current(window: tauri::WebviewWindow) -> Result<Option<Value>, String> {
    media_use::current(window.label())
}

#[tauri::command]
fn media_use_released(window: tauri::WebviewWindow, token: u64) -> Result<bool, String> {
    media_use::acknowledge(token, window.label())
}

#[tauri::command(async)]
fn background_work_snapshot(
    app: AppHandle,
) -> Result<background_work::BackgroundWorkSnapshot, String> {
    let data_root = paths::data_root(&app)?;
    background_work::snapshot(
        &data_root,
        derived_runtime::snapshot(derived_runtime::RuntimeConditions {
            busy: !derived_work::available(),
        })?,
        derived_work::work_capabilities(&data_root)?,
    )
}

#[tauri::command]
fn background_work_set_paused(
    app: AppHandle,
    class_id: Option<String>,
    paused: bool,
) -> Result<(), String> {
    derived_runtime::set_paused(&app, class_id.as_deref(), paused)?;
    if !paused {
        derived_work::start(app.clone())?;
    }
    derived_work::wake();
    Ok(())
}

/// Ephemeral viewport hints for the fixed derived-work coordinator. Output
/// facts remain the only queue; closing the app loses nothing that must be
/// recovered.
#[tauri::command]
fn prioritize_derived_work(
    selected_hash: Option<String>,
    visible_hashes: Vec<String>,
    section_kind: Option<String>,
    section_month: Option<String>,
) -> Result<(), String> {
    let section = match (section_kind, section_month) {
        (Some(kind), Some(month)) if matches!(kind.as_str(), "image" | "video") => {
            let bounds = queries::month_bounds(&month, display_timezone())?;
            Some(derived_work::SectionPriority {
                kind,
                start_ms: bounds.map(|value| value.0),
                end_ms: bounds.map(|value| value.1),
            })
        }
        _ => None,
    };
    derived_work::set_priority(selected_hash, visible_hashes, section);
    Ok(())
}

#[tauri::command]
fn transcribe_cancel() -> bool {
    derived_runtime::cancel_active_transcription()
}

// The Trash surface: standing sizes per trash root and the one deliberately
// destructive convenience — emptying a root. The trash is otherwise
// write-only; these are the only two readers the design allows.
#[tauri::command(async)]
fn trash_overview(app: AppHandle) -> Result<Vec<trash::TrashRootInfo>, String> {
    logging::boundary(
        "trash_overview",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let dirs = storage::load_config_source_dirs(&data_root)?;
            Ok(trash::overview(&dirs, &data_root))
        },
        |roots| json!({ "roots": roots.len() }),
    )
}

// Emptying is PERMANENT (the trash is the safety net; emptying it removes
// the net for everything inside). The frontend confirms with the totals
// before calling; the root path must be one `trash_overview` reported —
// verified here so the command can never delete an arbitrary tree.
#[tauri::command(async)]
fn trash_empty(app: AppHandle, root: String) -> Result<trash::EmptyOutcome, String> {
    mutation_runtime::empty_trash(&app, root)
}

#[tauri::command(async)]
fn trash_empty_cancel() -> Result<bool, String> {
    mutation_runtime::request_active_cancel()
}

// Dismissal is the user's half of the issues lifecycle: scan-derived rows
// clear themselves when a scan finds the condition resolved, these two clear
// everything else. Deleting is honest — the log file keeps the history, and a
// dismissed-but-persisting scan condition returns on the next scan.
#[tauri::command(async)]
fn dismiss_issue(app: AppHandle, id: i64) -> Result<(), String> {
    logging::boundary(
        "dismiss_issue",
        json!({ "id": id }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            conn.execute("DELETE FROM issues WHERE id = ?1", [id])
                .map_err(|e| e.to_string())?;
            Ok(())
        },
        |_| json!({}),
    )
}

#[tauri::command(async)]
fn dismiss_all_issues(app: AppHandle) -> Result<(), String> {
    logging::boundary(
        "dismiss_all_issues",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            conn.execute("DELETE FROM issues", [])
                .map_err(|e| e.to_string())?;
            Ok(())
        },
        |_| json!({}),
    )
}

#[tauri::command(async)]
fn retry_issue(app: AppHandle, id: i64) -> Result<bool, String> {
    logging::boundary(
        "retry_issue",
        json!({ "id": id }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            if issue_recovery::issue_has_kind(&conn, id, issue_recovery::DERIVED_WORKER_FAILED)? {
                derived_work::start(app.clone())?;
                derived_work::wake();
                return Ok(true);
            }
            if let Some(class) = derived_state::take_resource_issue(&conn, id)? {
                derived_runtime::set_paused(&app, Some(class.id()), false)?;
                derived_work::wake();
                return Ok(true);
            }
            let retried = derived_state::retry_issue(&conn, id)?;
            if retried {
                derived_work::wake();
            }
            Ok(retried)
        },
        |retried| json!({ "retried": retried }),
    )
}

#[tauri::command(async)]
fn retry_all_issues(app: AppHandle) -> Result<u64, String> {
    logging::boundary(
        "retry_all_issues",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let restart_derived =
                issue_recovery::contains_kind(&conn, issue_recovery::DERIVED_WORKER_FAILED)?;
            let mut retried = derived_state::retry_all(&conn)?;
            for class in derived_state::take_all_resource_issues(&conn)? {
                derived_runtime::set_paused(&app, Some(class.id()), false)?;
                retried += 1;
            }
            if restart_derived {
                derived_work::start(app.clone())?;
                retried += 1;
            }
            if retried > 0 {
                derived_work::wake();
            }
            Ok(retried)
        },
        |retried| json!({ "retried": retried }),
    )
}

#[tauri::command(async)]
fn recheck_issue(app: AppHandle, id: i64) -> Result<issue_recovery::RecheckResult, String> {
    logging::boundary(
        "recheck_issue",
        json!({ "id": id }),
        || {
            let data_root = paths::data_root(&app)?;
            let loaded = storage::load_app_data(&app)?;
            let settings = scanner::settings_from_config(
                loaded.config.as_ref(),
                &data_root,
                chrono::Utc::now().timestamp_millis(),
            );
            let db_file = data_root.join(storage::INDEX_DB_FILE_NAME);
            let outcome = scan_runtime::try_with_recheck_claim(id, || {
                let conn = index_store::open(&db_file)?;
                scanner::recheck_filesystem_issue(&conn, id, &settings.lists)
            })?;
            let Some(outcome) = outcome else {
                return Ok(issue_recovery::RecheckResult::Busy);
            };
            let include_walk = match outcome? {
                scanner::RecheckOutcome::Resolved { include_walk } => include_walk,
                scanner::RecheckOutcome::NotRecoverable => {
                    return Ok(issue_recovery::RecheckResult::NotRecoverable)
                }
                scanner::RecheckOutcome::StillFailing => {
                    return Ok(issue_recovery::RecheckResult::StillFailing)
                }
            };
            // Recheck itself is one bounded path/directory probe. Any durable
            // index debt it reveals resumes through the one existing worker,
            // with its normal cancellation and progress surface.
            if include_walk {
                let _ = source_check_runtime::start(app.clone())?;
            } else {
                file_information_runtime::wake(app.clone());
            }
            Ok(issue_recovery::RecheckResult::Started)
        },
        |result| json!({ "result": result }),
    )
}

// Every managed dependency's presence + facts + derived status, in display
// order — the Managed tools window renders one row per entry, and the ffmpeg
// chip reads its entry out of the same list.
#[tauri::command(async)]
fn binaries_state(app: AppHandle) -> Result<Vec<binaries_manager::DependencyState>, String> {
    let data_root = paths::data_root(&app)?;
    Ok(binaries_manager::states(&data_root))
}

#[derive(serde::Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
enum BinaryInstallResult {
    Installed {
        #[serde(rename = "operationId")]
        operation_id: String,
        state: binaries_manager::DependencyState,
    },
    Cancelled {
        #[serde(rename = "operationId")]
        operation_id: String,
        state: binaries_manager::DependencyState,
    },
    Failed {
        #[serde(rename = "operationId")]
        operation_id: String,
        state: binaries_manager::DependencyState,
        error: String,
    },
}

enum InstallWorkerOutcome {
    Installed,
    Cancelled,
    Failed(String),
}

// Installs or updates one registry entry on a blocking worker. Progress is an
// operation-correlated event; the awaited command response is the single
// authoritative terminal boundary, including a fresh artifact-derived row.
#[tauri::command(async)]
async fn binaries_install(
    app: AppHandle,
    id: String,
    operation_id: String,
) -> Result<BinaryInstallResult, String> {
    let data_root = paths::data_root(&app)?;
    let started = binaries_manager::begin_install(&id, &operation_id)?;
    let handle = app.clone();
    let worker_id = id.clone();
    let worker_operation_id = operation_id.clone();
    let worker_root = data_root.clone();
    let joined = tokio::task::spawn_blocking(move || {
        let progress_id = worker_id.clone();
        let progress_operation_id = worker_operation_id.clone();
        let progress_handle = handle.clone();
        let progress_event_failed = std::cell::Cell::new(false);
        let last_phase = std::cell::Cell::new(None::<binaries_manager::InstallPhase>);
        let last_emit =
            std::cell::Cell::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
        let emit = move |progress: binaries_manager::InstallProgress| {
            let now = std::time::Instant::now();
            let phase_changed = last_phase.get() != Some(progress.phase);
            let completed = progress.total.is_some_and(|total| progress.done >= total);
            if !phase_changed
                && !completed
                && now.duration_since(last_emit.get()) < std::time::Duration::from_millis(125)
            {
                return;
            }
            last_phase.set(Some(progress.phase));
            last_emit.set(now);
            if let Err(error) = failure_runtime::emit_checked(
                &progress_handle,
                "binaries://progress",
                json!({
                    "id": progress_id,
                    "operationId": progress_operation_id,
                    "phase": progress.phase,
                    "done": progress.done,
                    "total": progress.total,
                    "nextPhase": progress.next_phase,
                }),
            ) {
                if !progress_event_failed.replace(true) {
                    let _ = failure_runtime::report(
                        &progress_handle,
                        "event-delivery-failed",
                        Some(&progress_id),
                        &error,
                    );
                }
            }
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            binaries_manager::install_entry_started(&worker_root, started, emit)
        }));
        let outcome = match outcome {
            Ok(Ok(_facts)) => InstallWorkerOutcome::Installed,
            Ok(Err(error)) if binaries_manager::is_cancelled_error(&error) => {
                InstallWorkerOutcome::Cancelled
            }
            Ok(Err(error)) => InstallWorkerOutcome::Failed(error),
            Err(payload) => InstallWorkerOutcome::Failed(failure_runtime::panic_message(payload)),
        };
        let spec = binaries_manager::spec_of(&worker_id)
            .expect("a claimed dependency remains registered");
        let state = binaries_manager::state_of(&worker_root, spec);
        (outcome, state)
    })
    .await;

    let (outcome, state) = match joined {
        Ok(result) => result,
        Err(error) => {
            let message = format!("dependency install worker failed: {error}");
            let spec = binaries_manager::spec_of(&id)
                .ok_or_else(|| format!("unknown dependency: {id}"))?;
            (
                InstallWorkerOutcome::Failed(message),
                binaries_manager::state_of(&data_root, spec),
            )
        }
    };

    match outcome {
        InstallWorkerOutcome::Installed => {
            if let Err(error) =
                failure_runtime::clear(&app, "dependency-install-failed", Some(&id))
            {
                let _ = failure_runtime::report(
                    &app,
                    "issue-recovery-failed",
                    Some(&id),
                    &error,
                );
            }
            derived_work::wake();
            Ok(BinaryInstallResult::Installed {
                operation_id,
                state,
            })
        }
        InstallWorkerOutcome::Cancelled => {
            logging::info(
                "dependency install cancelled",
                json!({ "id": id, "operationId": operation_id }),
            );
            Ok(BinaryInstallResult::Cancelled {
                operation_id,
                state,
            })
        }
        InstallWorkerOutcome::Failed(error) => {
            logging::warn(
                "dependency install failed",
                json!({
                    "id": id,
                    "operationId": operation_id,
                    "error": { "message": error.clone() }
                }),
            );
            let _ = failure_runtime::report(
                &app,
                "dependency-install-failed",
                Some(&id),
                &error,
            );
            if state.status != binaries::BinaryStatus::NotInstalled {
                derived_work::wake();
            }
            Ok(BinaryInstallResult::Failed {
                operation_id,
                state,
                error,
            })
        }
    }
}

#[tauri::command(async)]
fn binaries_cancel(id: String, operation_id: String) -> bool {
    binaries_manager::cancel_entry(&id, &operation_id)
}

// Version check for one entry — never installs; a failure writes nothing.
#[derive(serde::Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
enum BinaryCheckOutcome {
    Completed {
        states: Vec<binaries_manager::DependencyState>,
    },
    Cancelled,
}

#[tauri::command(async)]
fn binaries_check(
    app: AppHandle,
    id: String,
    operation_id: String,
) -> Result<BinaryCheckOutcome, String> {
    logging::boundary(
        "binaries_check",
        json!({ "id": id }),
        || {
            let data_root = paths::data_root(&app)?;
            match binaries_manager::check_entry_with_operation(&data_root, &id, &operation_id) {
                Ok(_) => Ok(BinaryCheckOutcome::Completed {
                    states: binaries_manager::states(&data_root),
                }),
                Err(error) if error == binaries_acquisition::CANCELLED_ERROR => {
                    Ok(BinaryCheckOutcome::Cancelled)
                }
                Err(error) => Err(error),
            }
        },
        |outcome| match outcome {
            BinaryCheckOutcome::Completed { states } => {
                json!({ "outcome": "completed", "entries": states.len() })
            }
            BinaryCheckOutcome::Cancelled => json!({ "outcome": "cancelled" }),
        },
    )
}

// Wizard support: is this a real IANA timezone name?
#[tauri::command]
fn validate_timezone(name: String) -> bool {
    resolution::parse_timezone_name(&name).is_ok()
}

// The session gate's check: configured source directories that are not
// currently present (an unmounted volume manifests as a missing directory).
#[tauri::command(async)]
fn check_source_dirs(app: AppHandle) -> Result<SourceDirsStatus, String> {
    logging::boundary(
        "check_source_dirs",
        json!({}),
        || verify_source_dirs(&app),
        |status| json!({ "missing": status.missing.len(), "substituted": status.substituted.len() }),
    )
}

// The comparison view's group members for one item, best-first.
#[tauri::command(async)]
fn get_similar_group(app: AppHandle, hash: String) -> Result<Vec<queries::GroupMember>, String> {
    logging::boundary(
        "get_similar_group",
        json!({ "hash": hash }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let config = storage::read_config_for_setup(&data_root)?;
            let use_face_score = config
                .as_ref()
                .and_then(|value| value.get("scoreFaces"))
                .and_then(Value::as_bool)
                .unwrap_or_else(|| storage::DefaultConfig::default().score_faces);
            queries::similar_group_of(&conn, &hash, use_face_score)
        },
        |members| json!({ "members": members.len() }),
    )
}

#[tauri::command(async)]
fn comparison_live_hashes(app: AppHandle, hashes: Vec<String>) -> Result<Vec<String>, String> {
    logging::boundary(
        "comparison_live_hashes",
        json!({ "members": hashes.len() }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::live_content_hashes(&conn, &hashes)
        },
        |live| json!({ "live": live.len() }),
    )
}

// The comparison view's unlink: this image is not the same subject as its
// similar-family. Persistent — an excluded pair never regroups on any later
// rebuild — and non-destructive: no file is touched.
#[tauri::command(async)]
fn similar_unlink(app: AppHandle, hash: String) -> Result<u64, String> {
    logging::boundary(
        "similar_unlink",
        json!({ "hash": hash }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let written = similarity::unlink_from_group(&conn, &data_root, &hash)?;
            if written > 0 {
                derived_work::wake();
            }
            Ok(written)
        },
        |written| json!({ "exclusions": written }),
    )
}

// The Settings surface for the unlink store: how many verdicts exist, and the
// one way to take them all back. Without a visible count the exclusions would
// be an invisible permanent store — the kind of silence the app avoids.
#[tauri::command(async)]
fn similar_exclusions_count(app: AppHandle) -> Result<u64, String> {
    logging::boundary(
        "similar_exclusions_count",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            similar_exclusions::count(&data_root)
        },
        |n| json!({ "count": n }),
    )
}

#[tauri::command(async)]
fn similar_exclusions_clear(app: AppHandle) -> Result<u64, String> {
    logging::boundary(
        "similar_exclusions_clear",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let previous = similar_exclusions::count(&data_root)?;
            if previous == 0 {
                return similar_exclusions::clear(&data_root);
            }
            let config = storage::read_config_for_setup(&data_root)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let settings = derived_work::settings_from_config(config.as_ref(), &data_root)?;
            similarity::ensure_config_current(&conn, &settings.similarity)?;
            similarity::mark_all_buckets_dirty(&conn)?;
            let cleared = similar_exclusions::clear(&data_root)?;
            similarity::record_all_exclusions_change(
                &conn,
                &similar_exclusions::pairs(&data_root)?,
            )?;
            derived_work::wake();
            Ok(cleared)
        },
        |n| json!({ "cleared": n }),
    )
}

// The metadata pane's detail for one logical item.
#[tauri::command(async)]
fn get_item_detail(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
) -> Result<queries::ItemDetail, String> {
    logging::boundary(
        "get_item_detail",
        json!({ "hash": hash, "pathId": path_id }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::item_detail(&conn, hash.as_deref(), path_id)
        },
        |detail| json!({ "copies": detail.copy_paths.len() }),
    )
}

// Left-pane section counts (logical items per kind per month), bucketed in the
// OS display timezone.
#[tauri::command(async)]
fn get_section_counts(app: AppHandle) -> Result<queries::SectionCounts, String> {
    logging::boundary(
        "get_section_counts",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            queries::cached_section_counts(
                &data_root.join(storage::INDEX_DB_FILE_NAME),
                display_timezone(),
            )
        },
        |counts| {
            json!({
                "imageMonths": counts.images.len(),
                "videoMonths": counts.videos.len(),
                "otherMonths": counts.others.len(),
            })
        },
    )
}

// Receives a structured log object from the webview frontend and writes it to
// the session file (the frontend has no filesystem access of its own).
#[tauri::command]
fn log_event(entry: Value) {
    logging::emit_forwarded(entry);
}

// Reports whether developer-only `debug` logging is on, so the frontend can
// gate its own debug events identically (a dev build, or ONECOPY_DEBUG=1).
#[tauri::command]
fn logging_debug_enabled() -> bool {
    logging::debug_enabled()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Developer-only `debug` logging: on for a dev build, or when explicitly
    // requested via ONECOPY_DEBUG=1. Off (and compiled-quiet) in release.
    let debug_enabled = cfg!(debug_assertions)
        || std::env::var("ONECOPY_DEBUG")
            .map(|v| v == "1")
            .unwrap_or(false);

    let builder = tauri::Builder::default()
        // Process ownership is the FIRST plugin setup. Its OS file lock is the
        // atomic authority; a secondary routes activation to the owner and exits
        // before logs, stores, the index, watchers, or destructive commands start.
        .plugin(instance_owner::init());
    // The embedded WebDriver is a compile-time acceptance-test flavor. A
    // production build has neither the dependency feature nor this server.
    #[cfg(feature = "app-e2e")]
    let builder = builder
        .plugin(tauri_plugin_wdio::init())
        .plugin(tauri_plugin_wdio_webdriver::init());

    let app = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_uri_scheme_protocol("mediacache", |_ctx, request| {
            media_protocol::serve_cache(&request)
        })
        .register_uri_scheme_protocol("mediafile", |_ctx, request| {
            media_protocol::serve_original(&request)
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if !EXIT_REQUESTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    window.app_handle().exit(0);
                }
            }
        })
        .setup(move |app| {
            // Everything in this closure runs BEFORE the window appears, so it
            // is the launch latency. Each phase logs its cost so a slow start
            // is attributable from the session log alone.
            let setup_started = std::time::Instant::now();
            // Open the per-session log file under the app's own data dir. The Rust
            // core has filesystem access even though the webview is sandboxed, and
            // it routes through the single storage-root resolver (paths::data_root)
            // so the log directory and the data directory share one source of
            // truth and both honor ONECOPY_HOME.
            let data_root = paths::data_root(app.handle())?;
            let log_path = data_root
                .join(paths::LOGS_DIR_NAME)
                .join(logging::session_filename());
            logging::init(&log_path, debug_enabled);
            install_panic_hook();
            // Open the write-through data-backup store once, best-effort, under the
            // same ONECOPY_HOME-aware root. If it cannot open, one warn is logged
            // and recording is disabled for the session — it never blocks startup.
            backup_store::init(data_root.join(backup_store::BACKUPS_DB_FILE_NAME));

            // Materialize config.json from the canonical defaults when absent —
            // the populated-but-not-yet-used point, before any consumer reads it.
            storage::materialize_config_if_missing(&data_root)?;

            // Create/verify the index schema so a schema problem surfaces at
            // startup, not mid-scan. Phase 2 owns a long-lived connection; this
            // one closes on drop.
            drop(index_store::open(
                &data_root.join(storage::INDEX_DB_FILE_NAME),
            )?);

            // Download staging is crash debris by definition: wipe at launch.
            binaries_manager::reset_temp_dir(&data_root);

            // The cache always lives under the managed data root. Existing
            // external cache trees from older builds are deliberately left
            // untouched; they are reconstructible and no longer referenced.
            let setup_config = storage::read_config_for_setup(&data_root)?;
            let cache_root = data_root.join(storage::CACHE_DIR_NAME);
            DATA_ROOT
                .set(data_root.clone())
                .map_err(|_| "data root was initialized more than once".to_string())?;
            // One coordinator owns reconstructible media work; its optional
            // heavy classes run only while the user is away.
            if let Err(error) = derived_work::start(app.handle().clone()) {
                let _ = failure_runtime::report(
                    app.handle(),
                    issue_recovery::DERIVED_WORKER_FAILED,
                    None,
                    &error,
                );
            }
            // The sweep walks the ENTIRE cache tree, which grows with the
            // library — it was the launch's biggest fixed cost, paid before
            // the window could appear. It maintains a reconstructible cache
            // and nothing needs its result before first paint, so it runs on
            // a background thread (its own connection; WAL carries the
            // concurrency).
            {
                let db_path = data_root.join(storage::INDEX_DB_FILE_NAME);
                let cache = preview::CachePaths::new(cache_root);
                let handle = app.handle().clone();
                let clear_handle = handle.clone();
                let _ = failure_runtime::spawn_reported(
                    handle,
                    "onecopy-cache-sweep",
                    "cache-sweep-failed",
                    move || {
                        let started = std::time::Instant::now();
                        let conn = index_store::open(&db_path)?;
                        let removed = preview::startup_sweep(&conn, &cache)?;
                        if removed > 0 {
                            logging::info(
                                "cache sweep",
                                json!({ "removed": removed, "ms": started.elapsed().as_millis() as u64 }),
                            );
                        }
                        failure_runtime::clear(&clear_handle, "cache-sweep-failed", None)?;
                        Ok(())
                    },
                );
            }

            // The watcher: ON by default, best-effort, over the configured
            // source roots (the Camera Roll inflow case). Restart picks up
            // source-dir changes; correctness never depends on it.
            let watch_settings = scanner::settings_from_config(setup_config.as_ref(), &data_root, 0);
            watcher::start(app.handle().clone(), watch_settings.source_dirs);

            // The one update switch (managed-runtime-dependencies): when ON,
            // an INSTALLED tool is checked at launch, throttled to ~daily so
            // launches never hammer the endpoints. Default off; a failed
            // check writes nothing (`check_entry`'s own contract).
            let check_at_launch = setup_config
                .as_ref()
                .and_then(|c| c.get("checkUpdatesAtLaunch"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if check_at_launch {
                // The conventions' one toggle covers every installed entry
                // that HAS an upstream to ask — binaries. A model's version
                // is compiled into the app, so it is never checked (and
                // never stamped): `state_of` derives its latest from the pin.
                let root = data_root.clone();
                let handle = app.handle().clone();
                let stale_ids: Vec<String> = binaries_manager::states(&data_root)
                    .into_iter()
                    .filter(|entry| {
                        entry.checkable
                            && entry.status != binaries::BinaryStatus::NotInstalled
                            && entry
                                .facts
                                .last_checked_at_utc
                                .as_deref()
                                .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                                .map(|t| {
                                    chrono::Utc::now().signed_duration_since(t)
                                        > chrono::Duration::hours(24)
                                })
                                .unwrap_or(true)
                    })
                    .map(|entry| entry.id)
                    .collect();
                if !stale_ids.is_empty() {
                    let report_handle = handle.clone();
                    let _ = failure_runtime::spawn_reported(
                        handle,
                        "onecopy-update-check",
                        "update-check-worker-failed",
                        move || {
                            for id in stale_ids {
                                match binaries_manager::check_entry(&root, &id) {
                                    Ok(facts) => {
                                        failure_runtime::clear(
                                            &report_handle,
                                            "update-check-failed",
                                            Some(&id),
                                        )?;
                                        logging::info(
                                            "launch update check",
                                            json!({ "id": id, "latestKnown": facts.latest_known_version }),
                                        );
                                    }
                                    Err(error) => failure_runtime::report(
                                        &report_handle,
                                        "update-check-failed",
                                        Some(&id),
                                        &error,
                                    )?,
                                }
                            }
                            failure_runtime::emit_checked(
                                &report_handle,
                                "binaries://changed",
                                json!({}),
                            )?;
                            failure_runtime::clear(
                                &report_handle,
                                "update-check-worker-failed",
                                None,
                            )?;
                            Ok(())
                        },
                    );
                }
            }

            logging::info(
                "app startup",
                json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "build": if cfg!(debug_assertions) { "debug" } else { "release" },
                    "debugLogging": debug_enabled,
                    "logPath": log_path.to_string_lossy(),
                    "os": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                    // The pre-window cost. Anything slow after this line is a
                    // background thread, not launch latency.
                    "setupMs": setup_started.elapsed().as_millis() as u64,
                }),
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_app_data,
            patch_config,
            patch_state,
            record_interface_failure,
            start_source_check,
            stop_source_check,
            set_file_information_paused,
            admit_background_completion,
            index_work_snapshot,
            rebuild_library_index,
            get_section_counts,
            get_section_window,
            reconcile_section,
            get_section_range,
            get_section_family_context,
            viewer_sequence_start,
            viewer_sequence_move,
            viewer_sequence_reconcile,
            viewer_sequence_close,
            comparison_selection_valid,
            get_item_detail,
            get_similar_group,
            comparison_live_hashes,
            similar_unlink,
            similar_exclusions_count,
            similar_exclusions_clear,
            delete_items,
            mutation_cancel,
            move_items_out,
            list_subdirs,
            create_subdir,
            delete_empty_dir,
            reveal_data_subdir,
            open_item_externally,
            text_preview,
            text_encodings,
            media_use_current,
            media_use_released,
            note_user_activity,
            background_work_snapshot,
            background_work_set_paused,
            prioritize_derived_work,
            set_window_simple_fullscreen,
            ensure_preview,
            re_resolve_all,
            rescan_section,
            get_issues,
            get_recent_notifications,
            get_active_notifications,
            publish_notification,
            record_recent_notification,
            dismiss_notification,
            ensure_fullres,
            transcribe,
            transcript_get,
            transcribe_cancel,
            trash_overview,
            trash_empty,
            trash_empty_cancel,
            dismiss_issue,
            dismiss_all_issues,
            retry_issue,
            retry_all_issues,
            recheck_issue,
            binaries_state,
            binaries_install,
            binaries_cancel,
            binaries_check,
            validate_timezone,
            check_source_dirs,
            log_event,
            logging_debug_enabled
        ])
        .build(tauri::generate_context!());

    // A setup failure lands here — most consequentially a store that is unreadable for a
    // reason other than absence, or one that could not be set aside. OneCopy must not
    // reset over bytes it failed to preserve, so it halts; a panic into stderr is not a
    // halt for a double-clicked app, so the halt is reported natively before exiting
    // (storage-path conventions: a halt names the store and reaches the user).
    let app = match app {
        Ok(app) => app,
        Err(error) => {
            let message = error.to_string();
            logging::error("startup failed", json!({ "error": { "message": message } }));
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("OneCopy could not start")
                .set_description(format!(
                    "A settings file could not be read, and OneCopy could not set it aside either — so it \
                     has been left exactly where it is rather than risk overwriting it.\n\n{message}\n\n\
                     Your photos are not affected. Repair or move the file under the OneCopy data folder, \
                     then start OneCopy again."
                ))
                .show();
            std::process::exit(1);
        }
    };

    app.run(|app_handle, event| match event {
        // Cooperative scan interruption: flag as soon as exit is requested so
        // the worker starts winding down, then join it at Exit — bounded by
        // the per-item cancel checks — so no SQLite write is killed halfway.
        tauri::RunEvent::ExitRequested { api, .. } => {
            if EXIT_QUIESCING.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            EXIT_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
            api.prevent_exit();
            if EXIT_QUIESCING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            if let Err(error) = app_handle.emit("app://exit-quiescing", ()) {
                logging::warn(
                    "exit wait state delivery failed",
                    json!({ "error": { "message": error.to_string() } }),
                );
            }
            // A hidden or abruptly destroyed simple-fullscreen window can
            // leave macOS system chrome suppressed. Leave presentation mode
            // synchronously before the longer worker-quiescence shutdown.
            #[cfg(target_os = "macos")]
            for (label, window) in app_handle.webview_windows() {
                if label == "viewer" || label.starts_with("comparison-") {
                    if let Err(error) = window.set_simple_fullscreen(false) {
                        let _ = failure_runtime::report(
                            app_handle,
                            "shutdown-window-recovery-failed",
                            None,
                            &error.to_string(),
                        );
                    }
                }
            }
            source_check_runtime::shutdown(app_handle);
            file_information_runtime::shutdown(app_handle);
            binaries_manager::begin_shutdown();
            if let Err(error) = mutation_runtime::request_shutdown() {
                let _ = failure_runtime::report(app_handle, "shutdown-worker-failed", None, &error);
            }
            let handle = app_handle.clone();
            let exit_handle = handle.clone();
            let started = std::thread::Builder::new()
                .name("onecopy-exit-quiescence".to_string())
                .spawn(move || {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        source_check_runtime::join();
                        file_information_runtime::join();
                        binaries_manager::wait_for_idle();
                        if let Err(error) = mutation_runtime::wait_for_idle() {
                            let _ = failure_runtime::report(
                                &handle,
                                "shutdown-worker-failed",
                                None,
                                &error,
                            );
                        }
                        let media = media_use::begin(&handle, &[]);
                        if let Err(error) = &media {
                            let _ = failure_runtime::report(
                                &handle,
                                "shutdown-media-release-failed",
                                None,
                                error,
                            );
                        }
                        drop(media);
                    }));
                    if let Err(payload) = outcome {
                        let error = failure_runtime::panic_message(payload);
                        let _ = failure_runtime::report(
                            &handle,
                            "shutdown-worker-failed",
                            None,
                            &error,
                        );
                    }
                    handle.exit(0);
                });
            if let Err(error) = started {
                let message = format!("could not start shutdown worker: {error}");
                let _ = failure_runtime::report(
                    &exit_handle,
                    "shutdown-worker-failed",
                    None,
                    &message,
                );
                exit_handle.exit(1);
            }
        }
        tauri::RunEvent::Exit => {
            source_check_runtime::shutdown(app_handle);
            file_information_runtime::shutdown(app_handle);
            binaries_manager::begin_shutdown();
            source_check_runtime::join();
            file_information_runtime::join();
            binaries_manager::wait_for_idle();
            logging::info("app shutdown", json!({ "reason": "exit" }));
        }
        _ => {}
    });
}

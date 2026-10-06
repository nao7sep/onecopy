use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::menu::{OPEN_SETTINGS_MENU_ID, SAFE_QUIT_MENU_ID};

pub mod activity;
pub mod activity_history;
mod sqlite;
pub mod ai_acceleration;
pub mod ai_dependencies;
mod app_lifecycle;
mod sleep_prevention;
pub mod background_work;
pub mod backup_store;
mod binary_archive;
pub mod binaries;
mod binaries_acquisition;
pub mod binaries_manager;
mod copy_metadata;
pub mod derived_runtime;
pub mod derived_state;
pub mod derived_work;
mod destinations;
pub mod work_priority;
pub mod extensions;
pub mod file_names;
pub mod formats;
pub mod face;
pub mod failure_runtime;
pub mod file_identity;
pub mod i18n;
pub mod menu;
pub mod file_information_runtime;
pub mod fs_publish;
pub mod fs_recovery;
pub mod fullscreen;
mod github_release;
pub mod hashing;
pub mod index_store;
pub mod visibility;
pub mod visibility_index;
pub mod indexed_file;
pub mod information_attempts;
pub mod library_settings;
pub mod attempt_boundaries;
mod instance_owner;
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
pub mod progress_throttle;
pub mod preview;
pub mod queries;
pub mod records;
pub mod records_view;
pub mod records_window;
pub mod resolution;
pub mod resource_limits;
pub mod scan_runtime;
pub mod scanner;
pub mod similarity;
pub mod source_check_runtime;
pub mod source_check_state;
pub mod startup;
pub mod storage;
mod store_patch;
pub mod subprocess;
pub mod text_preview;
pub mod theme;
pub mod timestamps;
pub mod transcription;
pub mod restore;
pub mod trash;
pub mod video;
pub mod viewer_sequence;
pub mod volume;
pub mod volume_io;
pub mod watcher;
pub mod winpath;
pub mod window_placement;

// --- Commands ---
//
// Every fallible command runs inside logging::boundary(): one `debug` line at
// the start, one `info` (success) or `error` (failure) line with the elapsed
// duration. Expected outcomes are modeled as serde-tagged enums where they
// arise, not as errors.

// Config + state + data root in one startup round-trip.
// THREADING: Tauri dispatches a plain `#[tauri::command]` fn on the MAIN
// thread (`ExecutionContext::Blocking` in tauri-macros). On macOS the main
// thread also commits the window's layer updates, so a command doing file,
// network, subprocess or index work freezes the visible UI for its whole
// duration — which is how a 40 ms check came to feel like half a second of
// dead button.
//
// An `async fn` command instead runs on the async runtime
// (`ExecutionContext::Async`): Tauri polls its future on a tokio WORKER task,
// never the blocking pool and never the main thread. A synchronous body
// inlined into an `async fn` still blocks that worker, and the default
// multi-thread runtime has only one worker per logical CPU, so a handful of
// such commands can starve every other command's dispatch. `dispatch()`
// below is therefore the one place an `async fn` command may touch SQLite,
// the filesystem, a subprocess, or a contended lock: it runs the closure on
// `tauri::async_runtime::spawn_blocking`'s dedicated blocking-thread pool and
// awaits the join handle, so the worker is free the whole time. Every command
// below that touches the disk, the network, a subprocess, or the index is an
// `async fn` whose body runs through `dispatch()`; there is no longer a
// sync-fn-marked-`(async)` command anywhere in this file. Commands left as
// plain `#[tauri::command]` are pure, in-memory, and fast — atomic flag
// flips and bare state reads (logging_debug_enabled, transcribe_cancel,
// mutation_cancel, note_user_activity, ...) — or must keep strict call order
// (log_event). The get_* reads route through `dispatch()` too — their
// responses may arrive OUT OF ORDER, which the stores absorb with
// request-sequence guards (`staleGuard` in src/state/request-seq.ts): a
// 30k-item month query on the main thread was exactly the block a slow
// machine felt as a frozen window.
pub(crate) async fn dispatch<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(f).await {
        Ok(result) => result,
        Err(error) => Err(format!("background command worker failed: {error}")),
    }
}


#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum BootstrapData {
    Ready { data: storage::LoadedAppData },
    Blocked { failure: startup::StartupFailure },
}

#[tauri::command]
async fn load_app_data(
    startup: tauri::State<'_, startup::StartupGate>,
) -> Result<BootstrapData, String> {
    if let Some(failure) = startup.failure() {
        return Ok(BootstrapData::Blocked { failure });
    }
    dispatch(move || {
        logging::boundary(
            "load_app_data",
            json!({}),
            || {
                let mut data = storage::load_app_data()?;
                data.debug_enabled = logging::debug_enabled();
                Ok(BootstrapData::Ready { data })
            },
            |result| {
                let BootstrapData::Ready { data } = result else {
                    unreachable!("the blocked bootstrap returns before the logging boundary")
                };
                json!({
                    "hasState": data.state.is_some(),
                    "quarantines": data.quarantines.len(),
                })
            },
        )
    })
    .await
}

#[tauri::command]
async fn record_interface_failure(
    window: tauri::WebviewWindow,
    message: String,
) -> Result<(), String> {
    let app = window.app_handle().clone();
    let label = window.label().to_string();
    dispatch(move || failure_runtime::report(&app, "interface-failed", Some(&label), &message)).await
}

#[tauri::command]
async fn appearance_preferences(app: AppHandle) -> Result<Value, String> {
    dispatch(move || {
        logging::boundary(
            "appearance_preferences",
            json!({}),
            || {
                let preferences = storage::appearance_preferences(&paths::data_root()?)?;
                // The language the core settled on at launch, plus what the computer
                // asked for, so a window paints its first text in the right language
                // and formats dates the computer's way when they share a language.
                let state = app.state::<i18n::LanguageState>();
                Ok(storage::with_language_fields(preferences, &state))
            },
            |_| json!({}),
        )
    })
    .await
}

// Settings saves send their changed sets and state saves are patches; see
// `store_patch`.
#[tauri::command]
async fn save_config(app: AppHandle, changes: Value, report_failure: Option<bool>) -> Result<Value, String> {
    dispatch(move || store_patch::save_config(&app, changes, report_failure.unwrap_or(true))).await
}

#[tauri::command]
async fn patch_state(app: AppHandle, patch: Value, report_failure: Option<bool>) -> Result<Value, String> {
    dispatch(move || store_patch::patch_state(&app, &patch, report_failure.unwrap_or(true))).await
}

// The managed-tool update check's attempt timestamp lives in `dependencies.json`
// (the facts store), not in state.json; the frontend's manual check records it
// here. The core quietly returns the failure: the caller shows its own notice.
#[tauri::command]
async fn record_managed_tool_check_attempt() -> Result<(), String> {
    dispatch(|| {
        binaries_manager::save_check_attempt(
            &paths::data_root()?,
            binaries_manager::MANAGED_TOOL_UPDATE_ATTEMPT_KEY,
            &logging::now_iso_millis(),
        )
    })
    .await
}

#[tauri::command]
async fn start_source_check(app: AppHandle, explicit: bool) -> Result<bool, String> {
    dispatch(move || {
        let result = if explicit {
            source_check_runtime::start_explicit(app.clone())
        } else {
            source_check_runtime::start(app.clone())
        };
        result.map_err(|error| {
            if !app_lifecycle::shutting_down() {
                let _ = failure_runtime::report(&app, "source-check-failed", None, &error);
            }
            error
        })
    })
    .await
}

// Control command: only flips an in-memory stop flag (source_check_runtime's
// own state Mutex) and wakes the file-information worker; it does no index or
// filesystem work of its own, so it stays plain and immediate — a running
// scan's Cancel must never queue behind another command.
#[tauri::command]
fn stop_source_check(app: AppHandle) -> bool {
    source_check_runtime::stop(&app)
}

// Control commands: both only flip in-memory atomics/state (no index or
// filesystem access — see file_information_runtime::snapshot, which now
// reads a cached flag instead of probing the index) and spawning a worker
// thread is not itself blocking work, so they stay plain and immediate.
#[tauri::command]
fn set_file_information_paused(app: AppHandle, paused: bool) {
    file_information_runtime::set_paused(app, paused);
}

#[tauri::command]
fn admit_background_completion(app: AppHandle) {
    file_information_runtime::admit_background_completion(app);
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexWorkSnapshot {
    source_check: source_check_runtime::Snapshot,
    file_information: file_information_runtime::Snapshot,
}

// Both snapshots read in-memory/atomic state only (file_information_runtime's
// no longer opens the index — see its `snapshot()`), so this stays plain.
#[tauri::command]
fn index_work_snapshot() -> IndexWorkSnapshot {
    IndexWorkSnapshot {
        source_check: source_check_runtime::snapshot(),
        file_information: file_information_runtime::snapshot(),
    }
}

#[tauri::command]
async fn rebuild_library_index(
    app: AppHandle,
    discard_previews: bool,
    discard_transcripts: bool,
    discard_faces: bool,
) -> Result<(), String> {
    dispatch(move || {
        logging::boundary(
            "rebuild_library_index",
            json!({
                "discardPreviews": discard_previews,
                "discardTranscripts": discard_transcripts,
                "discardFaces": discard_faces,
            }),
            || mutation_runtime::rebuild_index(&app, discard_previews, discard_transcripts, discard_faces),
            |_| json!({}),
        )
    })
    .await
}

// Deletes an ordered logical-item set under one mutation/media boundary. The
// core plans every physical copy and companion before changing the first file;
// progress and cancellation belong to the shared ephemeral mutation runtime.
// This runs the entire batch inline (possibly hours of copying/hashing), so
// it must go through dispatch() rather than block a tokio worker.
#[tauri::command]
async fn delete_items(
    app: AppHandle,
    items: Vec<operations::ItemIdentity>,
    permanent: bool,
) -> Result<operations::DeleteBatchOutcome, String> {
    dispatch(move || mutation_runtime::delete_items(&app, items, permanent)).await
}

// Control command: flips an in-memory cancellation atomic only (see
// mutation_runtime::request_cancel); it must answer immediately so Cancel is
// never queued behind the running operation it is meant to stop.
#[tauri::command]
fn mutation_cancel(app: AppHandle, operation_id: u64) -> Result<bool, String> {
    mutation_runtime::request_cancel(operation_id).map_err(|error| {
        failure_runtime::report(&app, "file-operation-state-failed", None, &error)
            .err()
            .unwrap_or(error)
    })
}

#[tauri::command]
async fn get_section_window(
    kind: queries::SectionKind,
    month: String,
    sort: queries::SectionSort,
    start: u64,
    limit: u32,
) -> Result<queries::SectionWindow, String> {
    dispatch(move || {
        logging::boundary(
            "get_section_window",
            json!({ "kind": kind, "month": month, "start": start, "limit": limit }),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                queries::section_window(
                    &conn,
                    kind,
                    &month,
                    queries::display_timezone(),
                    sort,
                    start,
                    limit,
                    derived_work::item_projection(&data_root)?,
                )
            },
            |window| {
                json!({ "total": window.total, "start": window.start, "items": window.items.len() })
            },
        )
    })
    .await
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn reconcile_section(
    kind: queries::SectionKind,
    month: String,
    sort: queries::SectionSort,
    selected: Vec<queries::PositionedSectionIdentity>,
    anchor: Option<queries::SectionIdentity>,
    range_origin: Option<queries::SectionIdentity>,
    range_base: Vec<queries::SectionIdentity>,
    recovery: Option<queries::SectionRecoveryContext>,
    select_first: bool,
    limit: u32,
) -> Result<queries::SectionReconciliation, String> {
    dispatch(move || {
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
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                queries::reconcile_section(
                    &conn,
                    kind,
                    &month,
                    queries::display_timezone(),
                    sort,
                    &selected,
                    anchor.as_ref(),
                    range_origin.as_ref(),
                    &range_base,
                    recovery.as_ref(),
                    select_first,
                    limit,
                    derived_work::item_projection(&data_root)?,
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
    })
    .await
}

#[tauri::command]
async fn get_section_range(
    kind: queries::SectionKind,
    month: String,
    sort: queries::SectionSort,
    start: u64,
    end: u64,
) -> Result<Vec<queries::PositionedSectionIdentity>, String> {
    dispatch(move || {
        logging::boundary(
            "get_section_range",
            json!({ "kind": kind, "month": month, "start": start, "end": end }),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                queries::section_range(&conn, kind, &month, queries::display_timezone(), sort, start, end)
            },
            |items| json!({ "items": items.len() }),
        )
    })
    .await
}

#[tauri::command]
async fn get_section_family_context(
    kind: queries::SectionKind,
    month: String,
    sort: queries::SectionSort,
    member_hashes: Vec<String>,
) -> Result<Option<queries::SectionRecoveryContextOutput>, String> {
    dispatch(move || {
        logging::boundary(
            "get_section_family_context",
            json!({ "kind": kind, "month": month, "members": member_hashes.len() }),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                queries::section_family_context(
                    &conn,
                    kind,
                    &month,
                    queries::display_timezone(),
                    sort,
                    &member_hashes,
                )
            },
            |context| json!({ "found": context.is_some() }),
        )
    })
    .await
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn viewer_sequence_start(
    kind: queries::SectionKind,
    month: String,
    sort: queries::SectionSort,
    selected: Vec<queries::PositionedSectionIdentity>,
    anchor: queries::SectionIdentity,
) -> Result<viewer_sequence::Snapshot, String> {
    dispatch(move || {
        logging::boundary(
            "viewer_sequence_start",
            json!({ "kind": kind, "month": month, "selected": selected.len() }),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                viewer_sequence::start(
                    &data_root,
                    &conn,
                    kind,
                    &month,
                    queries::display_timezone(),
                    sort,
                    selected,
                    &anchor,
                    derived_work::item_projection(&data_root)?,
                )
            },
            |snapshot| json!({ "length": snapshot.length, "index": snapshot.index }),
        )
    })
    .await
}

#[tauri::command]
async fn viewer_sequence_move(
    token: String,
    movement: viewer_sequence::Move,
) -> Result<viewer_sequence::Snapshot, String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
        viewer_sequence::move_current(&token, movement, &conn, derived_work::item_projection(&data_root)?)
    })
    .await
}

#[tauri::command]
async fn viewer_sequence_reconcile(token: String) -> Result<Option<viewer_sequence::Snapshot>, String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        let index_db = data_root.join(storage::INDEX_DB_FILE_NAME);
        let conn = index_store::open(&index_db)?;
        viewer_sequence::reconcile(&token, &index_db, &conn, derived_work::item_projection(&data_root)?)
    })
    .await
}

// Control-like command: releases the sequence token from an in-memory map
// only (see viewer_sequence::close); no index or filesystem access.
#[tauri::command]
fn viewer_sequence_close(token: Option<String>) -> Result<(), String> {
    viewer_sequence::close(token.as_deref())
}

#[tauri::command]
async fn comparison_selection_valid(hashes: Vec<String>) -> Result<bool, String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
        queries::comparison_selection_valid(&conn, &hashes)
    })
    .await
}

#[tauri::command]
async fn get_item_section(identity: queries::SectionIdentity) -> Result<Option<queries::SectionLocation>, String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
        queries::section_for_identity(&conn, &identity, queries::display_timezone())
    })
    .await
}

#[tauri::command]
async fn resolve_library_path(path: String) -> Result<Option<queries::LibraryTarget>, String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
        queries::resolve_library_path(
            &conn,
            &path,
            &storage::configured_source_dirs(&data_root)?,
            queries::display_timezone(),
        )
    })
    .await
}

// Moves or copies one ordered logical-item set to a destination directory. Modes:
// "move-trash-rest" (plain drag), "move-delete-rest" (Shift), "copy" (Cmd/Ctrl).
// Destinations under a configured source root are rejected — moving files into
// a scanned directory would only re-index them.
#[tauri::command]
async fn move_items_out(
    app: AppHandle,
    items: Vec<operations::ItemIdentity>,
    dest_dir: String,
    mode: operations::MoveOutMode,
    conflict_policy: Option<operations::DestinationConflictPolicy>,
    plan_token: Option<String>,
) -> Result<operations::MoveBatchOutcome, String> {
    dispatch(move || {
        mutation_runtime::move_items_out(&app, items, dest_dir, mode, conflict_policy, plan_token)
    })
    .await
}

// Destination-tree support: see `destinations`.
#[tauri::command]
async fn list_subdirs(path: String) -> Result<Vec<destinations::DirEntry>, String> {
    dispatch(move || destinations::list_subdirs(&paths::data_root()?, &path)).await
}

#[tauri::command]
async fn create_subdir(parent: String, name: String) -> Result<String, String> {
    dispatch(move || {
        logging::boundary(
            "create_subdir",
            json!({ "parent": parent, "name": name }),
            || destinations::create_subdir(std::path::Path::new(&parent), &name),
            |path| json!({ "created": path }),
        )
    })
    .await
}

#[tauri::command]
async fn delete_empty_dir(path: String) -> Result<(), String> {
    dispatch(move || {
        logging::boundary(
            "delete_empty_dir",
            json!({ "path": path }),
            || destinations::delete_empty_dir(std::path::Path::new(&path)),
            |_| json!({}),
        )
    })
    .await
}

// Opens a subdirectory of the app's data root in the OS file manager (the
// "Reveal logs folder" menu item). The path is BUILT HERE from paths.rs and a
// vetted subdir name — the frontend names a folder, never a path (paths.rs:
// "the frontend never reconstructs ~/.onecopy itself"). Routing through the
// opener plugin's RUST api rather than its JS command also sidesteps the
// plugin's permission scope, which applies to webview calls only — the JS
// route silently rejected every path because the scope allow-list was empty,
// and a `void openPath(...)` threw the rejection away.
#[tauri::command]
async fn reveal_data_subdir(app: AppHandle, name: String) -> Result<(), String> {
    dispatch(move || {
        use tauri_plugin_opener::OpenerExt;
        let root = paths::data_root()?;
        let target = paths::revealable_data_subdir(&root, &name)?;
        app.opener()
            .open_path(target.to_string_lossy(), None::<&str>)
            .map_err(|e| e.to_string())
    })
    .await
}

// Opens an indexed item in its OS default app (the preview's "Open in player"
// codec-fallback). The path comes from the INDEX, never from the webview — a
// hash is resolved to a live copy here, so this can only ever open a file the
// scan actually indexed. Same reason as reveal_data_subdir for the Rust-side
// opener: the JS route was scope-rejected into a silent no-op, which left the
// fallback button for unplayable codecs doing nothing at all.
#[tauri::command]
async fn open_item_externally(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
) -> Result<(), String> {
    dispatch(move || {
        let result = logging::boundary(
            "open_item_externally",
            json!({ "hash": hash, "pathId": path_id }),
            || {
                use tauri_plugin_opener::OpenerExt;
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                let path = indexed_file::live_path(&conn, hash.as_deref(), path_id)?;
                let key = operations::ItemIdentity { hash, path_id }.key()?;
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
    })
    .await
}

#[tauri::command]
async fn text_preview(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
    encoding: Option<String>,
) -> Result<text_preview::PreviewBody, String> {
    dispatch(move || {
        let result = logging::boundary(
            "text_preview",
            json!({ "hash": hash, "pathId": path_id, "encoding": encoding }),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                let path = indexed_file::live_path(&conn, hash.as_deref(), path_id)?;
                let limits = text_preview::Limits::from_config(Some(&*storage::config(&data_root)?));
                text_preview::preview_file(&path, limits.max_bytes, &limits.fallback_encoding, encoding.as_deref())
            },
            |body| match body {
                text_preview::PreviewBody::Text {
                    byte_size,
                    encoding,
                    ..
                } => {
                    json!({ "body": "text", "byteSize": byte_size, "encoding": encoding })
                }
                text_preview::PreviewBody::Attributes {
                    byte_size,
                    reason,
                    reason_code,
                    reason_bytes,
                } => {
                    json!({
                        "body": "attributes",
                        "byteSize": byte_size,
                        "reason": reason,
                        "reasonCode": reason_code,
                        "reasonBytes": reason_bytes,
                    })
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
    })
    .await
}

#[tauri::command]
fn text_preview_options() -> text_preview::Options {
    text_preview::options()
}

#[tauri::command]
fn visibility_capabilities() -> visibility::Capabilities {
    visibility::capabilities()
}

// Publishes Settings-owned index projections; see
// scan_runtime::apply_library_settings.
#[tauri::command]
async fn apply_library_settings(app: AppHandle) -> Result<scan_runtime::LibrarySettingsOutcome, String> {
    dispatch(move || {
        logging::boundary(
            "apply_library_settings",
            json!({}),
            || scan_runtime::apply_library_settings(&app),
            |outcome| json!({ "outcome": outcome }),
        )
    })
    .await
}

// Scoped rescan of one section; see scan_runtime::recheck_section.
#[tauri::command]
async fn rescan_section(
    app: AppHandle,
    kind: queries::SectionKind,
    month: String,
) -> Result<scan_runtime::RescanSectionOutcome, String> {
    dispatch(move || {
        logging::boundary(
            "rescan_section",
            json!({ "kind": kind, "month": month }),
            || scan_runtime::recheck_section(&app, kind, &month, queries::display_timezone()),
            scan_runtime::RescanSectionOutcome::log_fields,
        )
    })
    .await
}

// The first-class issues surface: unreadable files, decode failures,
// copies-disagree anomalies, delete/copy errors — a silent skip never happens.
#[tauri::command]
async fn get_issues(
    limit: Option<u32>,
    after_first_seen_utc: Option<String>,
    after_id: Option<i64>,
) -> Result<serde_json::Value, String> {
    dispatch(move || {
        logging::boundary(
            "get_issues",
            json!({}),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                let cursor = match (after_first_seen_utc, after_id) {
                    (Some(first_seen_utc), Some(id)) => {
                        Some(queries::IssuesCursor { first_seen_utc, id })
                    }
                    _ => None,
                };
                let (total, rows) = queries::issues(&conn, limit.unwrap_or(500), cursor.as_ref())?;
                Ok(json!({ "total": total, "rows": rows }))
            },
            |v| json!({ "total": v.get("total") }),
        )
    })
    .await
}

// Control-like read: the ACTIVE notification list is an in-memory Mutex, no
// index or filesystem access.
#[tauri::command]
fn get_active_notifications() -> Vec<notifications::NotificationRecord> {
    notifications::active()
}

#[tauri::command]
async fn publish_notification(
    app: AppHandle,
    request: notifications::NotificationRequest,
) -> Result<notifications::NotificationRecord, String> {
    dispatch(move || notifications::publish(&app, request)).await
}

#[tauri::command]
async fn record_recent_notification(
    app: AppHandle,
    request: notifications::NotificationRequest,
) -> Result<notifications::NotificationRecord, String> {
    dispatch(move || {
        let record = notifications::record_history(request)?;
        failure_runtime::emit_or_record(&app, "notification://recorded", &record);
        Ok(record)
    })
    .await
}

// Control-like command: the ACTIVE list lives in memory (see
// notifications::dismiss); no index or filesystem access.
#[tauri::command]
fn dismiss_notification(app: AppHandle, id: i64) -> Result<bool, String> {
    notifications::dismiss(&app, id)
}

// On-demand derive for ONE clicked photo the scan's bulk pass has not reached
// (walk-order; on a slow machine its tail is hours away). The preview surface
// calls this when its cache entry 404s, then reloads the entry. Idempotent
// and cheap when the entry already exists.
#[tauri::command]
async fn ensure_preview(
    app: AppHandle,
    hash: String,
) -> Result<derived_work::EnsurePreviewResult, String> {
    dispatch(move || {
        // Registered so exit joins this ffmpeg-capable work instead of
        // orphaning it (W-L1) — see derived_work::RequestedMediaGuard.
        let _requested_media = derived_work::RequestedMediaGuard::begin();
        logging::boundary(
            "ensure_preview",
            json!({ "hash": hash }),
            || {
                let data_root = paths::data_root()?;
                let config = storage::config(&data_root)?;
                derived_work::ensure_preview(&app, &data_root, Some(&config), &hash)
            },
            |result| {
                json!({
                    "canonicalHash": result.canonical_hash,
                    "coalesced": result.coalesced,
                })
            },
        )
    })
    .await
}

// The 100% view's on-demand conversion for formats the webview cannot paint
// (HEIC/AVIF — WebView2 paints neither; routing every platform through the
// same path keeps behaviour identical and testable on macOS). The view calls
// this and then loads `mediacache://fullres-<hash>`.
#[tauri::command]
async fn ensure_fullres(app: AppHandle, hash: String) -> Result<(), String> {
    dispatch(move || {
        // See ensure_preview: registers this ffmpeg-capable work so exit
        // joins it instead of orphaning the process (W-L1).
        let _requested_media = derived_work::RequestedMediaGuard::begin();
        logging::boundary(
            "ensure_fullres",
            json!({ "hash": hash }),
            || derived_work::ensure_fullres(&app, &paths::data_root()?, &hash),
            |_| json!({}),
        )
    })
    .await
}

// On-demand transcription (Design: Video handling): minutes-long and
// memory-heavy, owned by `derived_work` on its own joined thread; progress,
// done, cancelled, and error arrive as events and `transcript_get` serves the
// result thereafter. Only the validation runs through dispatch().
#[tauri::command]
async fn transcribe(app: AppHandle, hash: String, replace: Option<bool>) -> Result<(), String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        derived_work::request_transcription(app, data_root, hash, replace.unwrap_or(false))
    })
    .await
}

// The transcript's explicit output state: ready, failed in this launch, or
// pending.
#[tauri::command]
async fn transcript_get(hash: String) -> Result<derived_state::TranscriptResult, String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
        derived_state::transcript_result(&conn, &hash)
    })
    .await
}

/// Main in Comparison and the fullscreen view: the window that holds the keyboard.
#[tauri::command]
fn set_window_fullscreen(app: AppHandle, label: String, enable: bool) -> Result<(), String> {
    fullscreen::set(&app, &label, enable, fullscreen::Surface::Focused)
}

/// One of Comparison's other displays.
#[tauri::command]
fn set_spread_fullscreen(app: AppHandle, label: String, enable: bool) -> Result<(), String> {
    fullscreen::set(&app, &label, enable, fullscreen::Surface::Spread)
}

#[tauri::command]
fn place_preview_window(
    app: AppHandle,
    normal: window_placement::NormalRectangle,
    maximized: bool,
    state: tauri::State<'_, window_placement::PreviewPlacementState>,
) -> Result<(), String> {
    let window = app
        .get_webview_window("preview")
        .ok_or("Preview window is unavailable")?;
    window_placement::place_preview(
        &window.as_ref().window(),
        normal,
        maximized,
        &state.0,
    )
}

#[tauri::command]
fn capture_preview_window_placement(
    app: AppHandle,
    state: tauri::State<'_, window_placement::PreviewPlacementState>,
) -> Result<(), String> {
    let window = app
        .get_webview_window("preview")
        .ok_or("Preview window is unavailable")?;
    window_placement::capture_preview(&window.as_ref().window(), &state.0);
    Ok(())
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

#[tauri::command]
async fn background_work_snapshot() -> Result<background_work::BackgroundWorkSnapshot, String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        background_work::snapshot(
            &data_root,
            derived_runtime::snapshot(derived_work::runtime_conditions())?,
            derived_work::work_capabilities(&data_root)?,
        )
    })
    .await
}

// Control command: derived_runtime::set_paused and file_information_runtime::set_paused
// only flip in-memory state (see their own comments); derived_work::start and
// ::wake only spawn/notify a worker thread, which is not itself blocking
// work. No index or filesystem access, so this stays plain and immediate.
#[tauri::command]
fn background_work_set_paused(
    app: AppHandle,
    class_id: Option<String>,
    paused: bool,
) -> Result<(), String> {
    derived_runtime::set_paused(&app, class_id.as_deref(), paused)?;
    if class_id.is_none() {
        file_information_runtime::set_paused(app.clone(), paused);
    }
    if !paused {
        derived_work::start(app.clone())?;
    }
    derived_work::wake();
    Ok(())
}

/// Ephemeral viewport hints for the fixed derived-work coordinator. Output
/// facts remain the only queue; closing the app loses nothing that must be
/// recovered. Resolving the section's month reads the system timezone, so it
/// runs through `dispatch()`; the hint generation already discards a response
/// that arrives after a newer one.
#[tauri::command]
async fn prioritize_derived_work(
    selected_hash: Option<String>,
    visible_hashes: Vec<String>,
    nearby_hashes: Vec<String>,
    section_kind: Option<queries::SectionKind>,
    section_month: Option<String>,
    section_sort: queries::SectionSort,
    section_anchor: u64,
    section_total: u64,
    generation: u64,
) -> Result<(), String> {
    dispatch(move || {
        let section = match (section_kind, section_month) {
            (Some(kind), Some(month)) => Some(derived_work::SectionPriority::for_month(
                kind,
                &month,
                queries::display_timezone(),
            )?),
            _ => None,
        };
        let traversal = section.as_ref().map(|_| derived_work::SectionTraversal {
            sort: section_sort, anchor: section_anchor, total: section_total,
        });
        derived_work::set_priority(selected_hash, visible_hashes, nearby_hashes, section, traversal, generation);
        Ok(())
    })
    .await
}

#[tauri::command]
fn transcribe_cancel() -> bool {
    derived_runtime::cancel_active_transcription()
}

// The Trash surface: standing sizes per trash root and the one deliberately
// destructive convenience — emptying a root. The trash is otherwise
// write-only; these are the only two readers the design allows.
#[tauri::command]
async fn trash_overview() -> Result<Vec<trash::TrashRootInfo>, String> {
    dispatch(move || {
        logging::boundary(
            "trash_overview",
            json!({}),
            || {
                let data_root = paths::data_root()?;
                let roots = storage::configured_file_roots(&data_root)?;
                Ok(trash::overview(&roots))
            },
            |roots| json!({ "roots": roots.len() }),
        )
    })
    .await
}

#[tauri::command]
async fn trash_reveal(app: AppHandle, root: String) -> Result<(), String> {
    dispatch(move || {
        use tauri_plugin_opener::OpenerExt;
        let data_root = paths::data_root()?;
        let roots = storage::configured_file_roots(&data_root)?;
        let path = std::path::PathBuf::from(&root);
        let path = trash::ensure_root_for_reveal(&roots, &path)?;
        app.opener()
            .open_path(path.to_string_lossy(), None::<&str>)
            .map_err(|error| error.to_string())
    })
    .await
}

// One configured root's deleted files, for Browse. Read only; the location
// must be one `trash_overview` reports for the current settings.
#[tauri::command]
async fn trash_entries(root: String) -> Result<trash::TrashListing, String> {
    dispatch(move || {
        logging::boundary(
            "trash_entries",
            json!({ "root": root }),
            || {
                let data_root = paths::data_root()?;
                let roots = storage::configured_file_roots(&data_root)?;
                let owning = trash::owning_root_of(&roots, std::path::Path::new(&root))?;
                trash::list_root(&owning, &data_root)
            },
            |listing| {
                json!({
                    "entries": listing.entries.len(),
                    "unrecorded": listing.unrecorded_files,
                    "malformed": listing.malformed_lines,
                })
            },
        )
    })
    .await
}

// Restores selected files from one root's deleted files back to their
// original paths (see `mutation_runtime::restore_entries`). Runs inline under
// the mutation claim, so it goes through dispatch(); cancel is
// `mutation_cancel`.
#[tauri::command]
async fn trash_restore(
    app: AppHandle,
    root: String,
    entries: Vec<String>,
    plan_token: Option<String>,
) -> Result<restore::RestoreOutcome, String> {
    dispatch(move || mutation_runtime::restore_entries(&app, root, entries, plan_token)).await
}

// Emptying is PERMANENT (the trash is the safety net; emptying it removes
// the net for everything inside). The frontend confirms with the totals
// before calling; the root path must be one `trash_overview` reported —
// verified here so the command can never delete an arbitrary tree. Runs the
// whole sweep inline, so it goes through dispatch() like the other mutations.
#[tauri::command]
async fn trash_empty(
    app: AppHandle,
    root: String,
    plan_token: String,
) -> Result<trash::EmptyOutcome, String> {
    dispatch(move || mutation_runtime::empty_trash(&app, root, plan_token)).await
}

// Control command: flips an in-memory cancellation atomic only (see
// mutation_runtime::request_active_cancel); stays plain and immediate.
#[tauri::command]
fn trash_empty_cancel() -> Result<bool, String> {
    mutation_runtime::request_active_cancel()
}

// Dismissal hides the live entry while retaining its diagnostic record.
#[tauri::command]
async fn dismiss_issue(id: i64) -> Result<(), String> {
    dispatch(move || {
        logging::boundary(
            "dismiss_issue",
            json!({ "id": id }),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                index_store::dismiss_issues(&conn, Some(id))
            },
            |_| json!({}),
        )
    })
    .await
}

#[tauri::command]
async fn dismiss_all_issues() -> Result<(), String> {
    dispatch(move || {
        logging::boundary(
            "dismiss_all_issues",
            json!({}),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                index_store::dismiss_issues(&conn, None)
            },
            |_| json!({}),
        )
    })
    .await
}

// Every managed dependency's presence + facts + derived status, in display
// order — the Managed tools window renders one row per entry, and the ffmpeg
// chip reads its entry out of the same list.
#[tauri::command]
async fn binaries_state() -> Result<Vec<binaries_manager::DependencyState>, String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        Ok(binaries_manager::states(&data_root))
    })
    .await
}

// Installs or updates one registry entry on a blocking worker; see
// binaries_manager::install_reported.
#[tauri::command]
async fn binaries_install(
    app: AppHandle,
    id: String,
    operation_id: String,
) -> Result<binaries_manager::InstallResult, String> {
    dispatch(move || {
        let data_root = paths::data_root()?;
        binaries_manager::install_reported(&app, &data_root, &id, &operation_id)
    })
    .await
}

// Control command: flips an in-memory cancellation atomic in the IN_FLIGHT
// registry only (see binaries_manager::cancel_entry); stays plain.
#[tauri::command]
fn binaries_cancel(id: String, operation_id: String) -> bool {
    binaries_manager::cancel_entry(&id, &operation_id)
}

// Version check for one entry — never installs; a failure writes nothing.
#[tauri::command]
async fn binaries_check(
    id: String,
    operation_id: String,
) -> Result<binaries_manager::CheckOutcome, String> {
    dispatch(move || {
        logging::boundary(
            "binaries_check",
            json!({ "id": id }),
            || {
                let data_root = paths::data_root()?;
                binaries_manager::check_reported(&data_root, &id, &operation_id)
            },
            binaries_manager::CheckOutcome::log_fields,
        )
    })
    .await
}

// The session gate's check: configured source directories that are not
// currently present (an unmounted volume manifests as a missing directory).
#[tauri::command]
async fn check_source_dirs() -> Result<volume::SourceDirsStatus, String> {
    let result = dispatch(move || volume::verify_source_dirs(&paths::data_root()?)).await;
    logging::boundary(
        "check_source_dirs",
        json!({}),
        || result,
        |status| json!({ "missing": status.missing.len(), "substituted": status.substituted.len() }),
    )
}

// The comparison view's group members for one item, best-first.
#[tauri::command]
async fn get_similar_group(hash: String) -> Result<Vec<queries::GroupMember>, String> {
    dispatch(move || {
        logging::boundary(
            "get_similar_group",
            json!({ "hash": hash }),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                let use_face_score = storage::config(&data_root)?
                    .get("scoreFaces")
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| storage::DefaultConfig::default().score_faces);
                queries::similar_group_of(&conn, &hash, use_face_score)
            },
            |members| json!({ "members": members.len() }),
        )
    })
    .await
}

#[tauri::command]
async fn comparison_live_hashes(hashes: Vec<String>) -> Result<Vec<String>, String> {
    dispatch(move || {
        logging::boundary(
            "comparison_live_hashes",
            json!({ "members": hashes.len() }),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                queries::live_content_hashes(&conn, &hashes)
            },
            |live| json!({ "live": live.len() }),
        )
    })
    .await
}

// The metadata pane's detail for one logical item.
#[tauri::command]
async fn get_item_detail(
    hash: Option<String>,
    path_id: Option<i64>,
) -> Result<queries::ItemDetail, String> {
    dispatch(move || {
        logging::boundary(
            "get_item_detail",
            json!({ "hash": hash, "pathId": path_id }),
            || {
                let data_root = paths::data_root()?;
                let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
                queries::item_detail(&conn, hash.as_deref(), path_id)
            },
            |detail| json!({ "copies": detail.copy_paths.len() }),
        )
    })
    .await
}

// Left-pane section counts (logical items per kind per month), bucketed in the
// OS display timezone.
#[tauri::command]
async fn get_section_counts() -> Result<queries::SectionCounts, String> {
    dispatch(move || {
        logging::boundary(
            "get_section_counts",
            json!({}),
            || {
                let data_root = paths::data_root()?;
                queries::cached_section_counts(
                    &data_root.join(storage::INDEX_DB_FILE_NAME),
                    queries::display_timezone(),
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
    })
    .await
}

// Receives a structured log object from the webview frontend and writes it to
// the session file (the frontend has no filesystem access of its own). Left
// as a plain main-thread command DELIBERATELY: log lines must keep the exact
// order the frontend sent them in, and dispatch()ing this one onto the
// blocking pool would let calls race each other and interleave out of order.
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

#[tauri::command]
async fn activity_record(
    draft: activity::ActivityDraft,
) -> Result<Option<activity::ActivityEvent>, String> {
    dispatch(move || activity::record(draft)).await
}

#[tauri::command]
async fn activity_page(
    before: Option<i64>,
    after: Option<i64>,
    limit: Option<usize>,
) -> Result<activity_history::OperationPage, String> {
    dispatch(move || {
        let mut page = activity::operations(before, after, limit)?;
        if page.operations.iter().any(|row| row.target_hash.is_some()) {
            let conn = index_store::open(&paths::data_root()?.join(storage::INDEX_DB_FILE_NAME))?;
            activity_history::resolve_targets(&conn, &mut page.operations)?;
        }
        Ok(page)
    })
    .await
}

#[tauri::command]
async fn activity_events(
    operation: i64,
    before: Option<i64>,
    limit: Option<usize>,
) -> Result<activity::ActivityPage, String> {
    dispatch(move || activity::events(operation, before, limit)).await
}

// The Records window (records_window.rs). Its reads log nothing on success:
// each log line is a stored record whose signal would start the next read.
#[tauri::command]
async fn open_records_window(app: AppHandle) -> Result<(), String> {
    dispatch(move || records_window::open(&app)).await
}

fn records_reader() -> Result<rusqlite::Connection, String> {
    records_view::open_reader(&paths::data_root()?.join(records::RECORDS_DB_FILE_NAME))
}

#[tauri::command]
async fn records_page(query: records_view::RecordsQuery) -> Result<records_view::RecordsPage, String> {
    dispatch(move || records_view::page(&records_reader()?, &query)).await
}

#[tauri::command]
async fn records_detail(
    kind: records_view::RecordKind,
    id: i64,
) -> Result<Option<records_view::RecordDetail>, String> {
    dispatch(move || records_view::detail(&records_reader()?, kind, id)).await
}

#[tauri::command]
async fn records_sources() -> Result<records_view::RecordSources, String> {
    dispatch(|| records_view::sources(&records_reader()?, logging::session_id())).await
}

// The list pane's saved width in `state.json`, read before the window's first
// frame; a drag's end saves it through `patch_state`.
#[tauri::command]
async fn records_list_width(app: AppHandle) -> Result<Option<f64>, String> {
    dispatch(move || {
        let state = store_patch::read_state(&app)?;
        Ok(state.and_then(|state| state.get("recordsListWidth").and_then(Value::as_f64)))
    })
    .await
}

// Control-like command: hands off to Tauri's own exit machinery, which the
// ExitRequested/Exit handlers below drive; no index or filesystem access here.
#[tauri::command]
fn request_app_exit(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
async fn check_github_release(app: AppHandle) -> Result<github_release::ReleaseCheckOutcome, String> {
    let result = github_release::check(&app).await;
    if let Err(error) = &result {
        logging::error(
            "GitHub release check could not start",
            json!({ "error": { "message": error } }),
        );
    }
    result
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Developer-only `debug` logging: on for a dev build, or when explicitly
    // requested via ONECOPY_DEBUG=1. Off (and compiled-quiet) in release.
    let debug_enabled = cfg!(debug_assertions)
        || std::env::var("ONECOPY_DEBUG")
            .map(|v| v == "1")
            .unwrap_or(false);

    // The interface language is resolved before Tauri builds the app: AppKit
    // settles its own language when the application object is created, and the
    // native menu is built from this reading. The theme comes from the same
    // held settings in setup, before Main is shown.
    let settings = paths::data_root_before_launch().map(|root| storage::held_config(&root));
    let language = i18n::LanguageState::detect(settings.as_ref());
    let launch_language = language.current();
    #[cfg(target_os = "macos")]
    i18n::align_appkit(language.current());

    let placement_state = window_placement::new_state();
    let preview_placement_state = window_placement::new_state();
    let records_placement_state = window_placement::new_state();
    let event_placement_state = placement_state.clone();
    let event_preview_placement_state = preview_placement_state.clone();
    let event_records_placement_state = records_placement_state.clone();
    let setup_placement_state = placement_state.clone();
    let setup_preview_placement_state = preview_placement_state.clone();
    let setup_records_placement_state = records_placement_state.clone();
    let builder = tauri::Builder::default()
        // Process ownership is the FIRST plugin setup. Its OS file lock is the
        // atomic authority; a secondary routes activation to the owner and exits
        // before logs, stores, the index, watchers, or destructive commands start.
        .plugin(instance_owner::init());
    let app = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(window_placement::PreviewPlacementState(
            preview_placement_state.clone(),
        ))
        .manage(window_placement::RecordsPlacementState(
            records_placement_state.clone(),
        ))
        .manage(theme::ThemeState::default())
        .manage(language)
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if let Some(session_event) = fullscreen::session_close_event(window.label()) {
                    api.prevent_close();
                    if let Err(error) = window.emit_to("main", session_event, ()) {
                        logging::warn(
                            "session window close request could not reach Main",
                            json!({ "window": window.label(), "error": { "message": error.to_string() } }),
                        );
                    }
                }
            }
            window_placement::on_window_event(
                window,
                event,
                &event_placement_state,
                &event_preview_placement_state,
                &event_records_placement_state,
            );
            // Under System the OS appearance can change while the app runs; keep
            // the backing behind each page in step. (macOS reports only OS
            // changes here, which is why theme::apply_to_webview sets it too.)
            if let tauri::WindowEvent::ThemeChanged(changed) = event {
                if let Err(error) =
                    window.set_background_color(Some(theme::window_background(*changed)))
                {
                    logging::warn(
                        "window background update failed",
                        json!({ "error": { "message": error.to_string() } }),
                    );
                }
            }
        })
        // Every window opened after launch — Preview, Comparison, the fullscreen view —
        // takes the recorded theme as its page starts loading, before it paints.
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Started {
                let current = webview.state::<theme::ThemeState>().current();
                if let Err(error) = theme::apply_to_webview(webview, current) {
                    logging::warn(
                        "window theme could not be applied",
                        json!({ "window": webview.label(), "error": { "message": error } }),
                    );
                }
                if let Err(error) = fullscreen::refuse_spaces_fullscreen(&webview.window()) {
                    logging::warn(
                        "window could not refuse Spaces fullscreen",
                        json!({ "window": webview.label(), "error": { "message": error } }),
                    );
                }
            }
        })
        .menu(|app| {
            let language = app.state::<i18n::LanguageState>().current();
            menu::build(app, language)
        })
        .on_menu_event(|app, event| {
            if event.id() == SAFE_QUIT_MENU_ID {
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(error) = window.close() {
                        logging::warn(
                            "route quit through main window failed",
                            json!({ "error": { "message": error.to_string() } }),
                        );
                    }
                } else {
                    app.exit(0);
                }
            } else if event.id() == OPEN_SETTINGS_MENU_ID {
                // The native item cannot call into the webview directly; Main
                // owns the only Settings surface, the same one the webview's
                // own Cmd+, shortcut already opens (R8-04).
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(error) = window.set_focus() {
                        logging::warn(
                            "focusing Main for the native Settings item failed",
                            json!({ "error": { "message": error.to_string() } }),
                        );
                    }
                }
                failure_runtime::emit_or_record(app, "menu://open-settings", json!({}));
            }
        })
        // Asynchronous registration (W-B2, C-B1): the handler body — index
        // open, the source/cache read, Range slicing — runs on the blocking
        // pool via spawn_blocking rather than inline on the main thread, so a
        // sleeping drive or a slow NAS no longer freezes every window. Range
        // and cap policy are unchanged; only the thread it runs on moved.
        .register_asynchronous_uri_scheme_protocol("mediacache", |_ctx, request, responder| {
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(media_protocol::answer(|| media_protocol::serve_cache(&request)));
            });
        })
        .register_asynchronous_uri_scheme_protocol("mediafile", |_ctx, request, responder| {
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(media_protocol::answer(|| media_protocol::serve_original(&request)));
            });
        })
        .setup(move |app| {
            // Tauri panics when this hook returns Err; on macOS that panic
            // crosses a callback that cannot unwind and becomes SIGABRT. The
            // application bootstrap therefore records Ready/Blocked state and
            // the hook itself is deliberately infallible.
            // Before the records open, so no stored record goes unsignalled.
            let records_app = app.handle().clone();
            records::on_stored(move || records_window::notify_changed(&records_app));
            app.manage(startup::initialize(app, debug_enabled));
            window_placement::load_preview(&setup_preview_placement_state);
            window_placement::load_records(&setup_records_placement_state);
            let saved_theme = settings.as_ref().and_then(theme::config_window_theme);
            app.state::<theme::ThemeState>().set(saved_theme);
            if let Some(window) = app.get_webview_window("main") {
                // Before Main is shown, so its first frame and title bar already
                // match the saved choice.
                if let Err(error) = theme::apply_to_webview(window.as_ref(), saved_theme) {
                    logging::warn(
                        "saved theme could not be applied to Main",
                        json!({ "error": { "message": error } }),
                    );
                }
                window_placement::restore(
                    &window.as_ref().window(),
                    &setup_placement_state,
                );
                if let Err(error) = window.show() {
                    logging::warn(
                        "Main window could not be shown",
                        json!({ "error": { "message": error.to_string() } }),
                    );
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_app_data,
            save_config,
            appearance_preferences,
            patch_state,
            record_managed_tool_check_attempt,
            record_interface_failure,
            start_source_check,
            stop_source_check,
            set_file_information_paused,
            admit_background_completion,
            index_work_snapshot,
            rebuild_library_index,
            get_section_counts,
            get_section_window,
            get_item_section,
            resolve_library_path,
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
            delete_items,
            mutation_cancel,
            move_items_out,
            list_subdirs,
            create_subdir,
            delete_empty_dir,
            reveal_data_subdir,
            open_item_externally,
            text_preview,
            text_preview_options,
            media_use_current,
            media_use_released,
            note_user_activity,
            background_work_snapshot,
            background_work_set_paused,
            prioritize_derived_work,
            set_window_fullscreen,
            set_spread_fullscreen,
            place_preview_window,
            capture_preview_window_placement,
            ensure_preview,
            apply_library_settings,
            visibility_capabilities,
            rescan_section,
            get_issues,
            get_active_notifications,
            publish_notification,
            record_recent_notification,
            dismiss_notification,
            ensure_fullres,
            transcribe,
            transcript_get,
            transcribe_cancel,
            trash_overview,
            trash_reveal,
            trash_entries,
            trash_restore,
            trash_empty,
            trash_empty_cancel,
            dismiss_issue,
            dismiss_all_issues,
            binaries_state,
            binaries_install,
            binaries_cancel,
            binaries_check,
            check_source_dirs,
            log_event,
            logging_debug_enabled,
            activity_record,
            activity_page,
            activity_events,
            open_records_window,
            records_page,
            records_detail,
            records_sources,
            records_list_width,
            request_app_exit,
            check_github_release
        ])
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(error) => startup::halt_before_runtime(&error.to_string(), launch_language),
    };

    app.run(move |app_handle, event| match event {
        tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::Focused(_),
            ..
        } => fullscreen::note_focus_transition(),
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } => fullscreen::window_destroyed(&label),
        tauri::RunEvent::MainEventsCleared => {
            if let Err(error) = fullscreen::reconcile_activation(app_handle) {
                logging::warn(
                    "fullscreen activation reconciliation failed",
                    json!({ "error": { "message": error } }),
                );
            }
        }
        // Every exit request first captures placement. Until the exit
        // sequence reports that exiting is safe, the request is prevented and
        // joins the one sequence (`app_lifecycle::quiesce`), which ends by
        // requesting exit itself.
        tauri::RunEvent::ExitRequested { api, .. } => {
            if let Some(window) = app_handle.get_webview_window("main") {
                window_placement::capture(&window.as_ref().window(), &placement_state);
            }
            if let Some(window) = app_handle.get_webview_window("preview") {
                window_placement::capture_preview(
                    &window.as_ref().window(),
                    &preview_placement_state,
                );
            }
            if let Some(window) = app_handle.get_webview_window(records_window::LABEL) {
                window_placement::capture(&window.as_ref().window(), &records_placement_state);
            }
            if app_lifecycle::exit_ready() {
                return;
            }
            api.prevent_exit();
            app_lifecycle::quiesce(app_handle);
        }
        tauri::RunEvent::Exit => {
            window_placement::save(&placement_state);
            window_placement::save_preview(&preview_placement_state);
            window_placement::save_records(&records_placement_state);
            logging::info("app shutdown", json!({ "reason": "exit" }));
        }
        // A Dock click brings a minimized Main back even while another window,
        // such as Records, is still showing; AppKit itself only does so when
        // no window is visible.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => {
            if let Some(window) = app_handle.get_webview_window("main") {
                let away = window.is_minimized().unwrap_or(false) || !window.is_visible().unwrap_or(true);
                if away {
                    if let Err(error) = window
                        .unminimize()
                        .and_then(|()| window.show())
                        .and_then(|()| window.set_focus())
                    {
                        logging::warn(
                            "Main could not be brought back on reopen",
                            json!({ "error": { "message": error.to_string() } }),
                        );
                    }
                }
            }
        }
        _ => {}
    });
}

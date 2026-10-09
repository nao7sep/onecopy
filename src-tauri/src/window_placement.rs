use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{PhysicalPosition, PhysicalSize, Window, WindowEvent, Wry};

use crate::{fullscreen, logging, paths, storage};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalRectangle {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub normal: NormalRectangle,
    pub maximized: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClosingState {
    Normal(NormalRectangle),
    Maximized,
    Transient,
}

pub fn placement_after_close(
    previous: Option<Placement>,
    closing: ClosingState,
    remember_maximized: bool,
) -> Option<Placement> {
    match closing {
        ClosingState::Normal(normal) => Some(Placement {
            normal,
            maximized: false,
        }),
        ClosingState::Maximized => previous.map(|placement| Placement {
            normal: placement.normal,
            maximized: remember_maximized,
        }),
        ClosingState::Transient => previous,
    }
}

pub fn frame_fills_work_area(frame: NormalRectangle, work_area: NormalRectangle) -> bool {
    frame.x.abs_diff(work_area.x) <= 2
        && frame.y.abs_diff(work_area.y) <= 2
        && frame.width.abs_diff(work_area.width) <= 2
        && frame.height.abs_diff(work_area.height) <= 2
}

/// The smallest on-screen sliver a restored window must show in each
/// dimension to count as "restored safely". Enough to see
/// and drag the title bar back into view; a 1px overlap after an unplugged
/// or resized monitor is not.
const MIN_RESTORE_OVERLAP: i64 = 80;

/// Whether `rectangle` overlaps `work_area` by a usable amount in both
/// dimensions, not merely a nonzero sliver.
pub fn restorable_overlap(rectangle: NormalRectangle, work_area: NormalRectangle) -> bool {
    if rectangle.width == 0 || rectangle.height == 0 {
        return false;
    }
    let left = i64::from(rectangle.x);
    let top = i64::from(rectangle.y);
    let right = left + i64::from(rectangle.width);
    let bottom = top + i64::from(rectangle.height);
    let area_left = i64::from(work_area.x);
    let area_top = i64::from(work_area.y);
    let area_right = area_left + i64::from(work_area.width);
    let area_bottom = area_top + i64::from(work_area.height);
    let overlap_width = right.min(area_right) - left.max(area_left);
    let overlap_height = bottom.min(area_bottom) - top.max(area_top);
    overlap_width >= MIN_RESTORE_OVERLAP && overlap_height >= MIN_RESTORE_OVERLAP
}

pub(crate) type PlacementState = Arc<Mutex<Option<Placement>>>;

pub(crate) struct PreviewPlacementState(pub PlacementState);

pub(crate) struct RecordsPlacementState(pub PlacementState);

pub(crate) fn new_state() -> PlacementState {
    Arc::new(Mutex::new(None))
}

pub(crate) fn load_records(state: &PlacementState) {
    let saved = paths::data_root()
        .and_then(|root| storage::read_records_window_state_for_setup(&root))
        .map(|value| value.and_then(|value| serde_json::from_value(value).ok()));
    match saved {
        Ok(saved) => set_state(state, saved),
        Err(error) => warn("Records window placement could not be loaded", error),
    }
}

pub(crate) fn load_preview(state: &PlacementState) {
    let saved = paths::data_root()
        .and_then(|root| storage::read_preview_window_state_for_setup(&root))
        .map(|value| value.and_then(|value| serde_json::from_value(value).ok()));
    match saved {
        Ok(saved) => set_state(state, saved),
        Err(error) => warn("Preview window placement could not be loaded", error),
    }
}

fn current_rectangle(window: &Window<Wry>) -> tauri::Result<NormalRectangle> {
    let position = window.outer_position()?;
    let size = window.inner_size()?;
    Ok(NormalRectangle {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    })
}

#[cfg(target_os = "macos")]
fn is_maximized(window: &Window<Wry>) -> tauri::Result<bool> {
    let position = window.outer_position()?;
    let size = window.outer_size()?;
    Ok(window.available_monitors()?.iter().any(|monitor| {
        let area = monitor.work_area();
        frame_fills_work_area(
            NormalRectangle {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
            },
            NormalRectangle {
                x: area.position.x,
                y: area.position.y,
                width: area.size.width,
                height: area.size.height,
            },
        )
    }))
}

#[cfg(not(target_os = "macos"))]
fn is_maximized(window: &Window<Wry>) -> tauri::Result<bool> {
    window.is_maximized()
}

fn usable(window: &Window<Wry>, rectangle: NormalRectangle) -> tauri::Result<bool> {
    Ok(window.available_monitors()?.iter().any(|monitor| {
        let area = monitor.work_area();
        restorable_overlap(
            rectangle,
            NormalRectangle {
                x: area.position.x,
                y: area.position.y,
                width: area.size.width,
                height: area.size.height,
            },
        )
    }))
}

fn set_state(state: &PlacementState, placement: Option<Placement>) {
    match state.lock() {
        Ok(mut current) => *current = placement,
        Err(error) => warn("window placement lock is unavailable", error),
    }
}

fn state_value(state: &PlacementState) -> Option<Placement> {
    match state.lock() {
        Ok(current) => *current,
        Err(error) => {
            warn("window placement lock is unavailable", error);
            None
        }
    }
}

fn warn(message: &str, error: impl ToString) {
    logging::warn(
        message,
        serde_json::json!({ "error": { "message": error.to_string() } }),
    );
}

pub(crate) fn restore(window: &Window<Wry>, state: &PlacementState) {
    let saved = paths::data_root()
        .and_then(|root| storage::read_window_state_for_setup(&root))
        .map(|value| value.and_then(|value| serde_json::from_value(value).ok()));
    let saved = match saved {
        Ok(saved) => saved,
        Err(error) => {
            warn("window placement could not be loaded", error);
            None
        }
    };
    apply_saved(window, state, saved);
}

/// Places the Records window, built hidden at its designed size, from the
/// placement this launch loaded or last captured from it.
pub(crate) fn place_records(window: &Window<Wry>, state: &PlacementState) {
    apply_saved(window, state, state_value(state));
}

/// Applies a saved normal rectangle, and on Windows its maximized state, to a
/// window that is still hidden; unusable placement leaves the designed size
/// and the toolkit's placement.
fn apply_saved(window: &Window<Wry>, state: &PlacementState, saved: Option<Placement>) {
    let fallback = current_rectangle(window).ok().map(|normal| Placement {
        normal,
        maximized: false,
    });
    let usable = saved.and_then(
        |placement: Placement| match usable(window, placement.normal) {
            Ok(true) => Some(placement),
            Ok(false) => None,
            Err(error) => {
                warn("window placement could not be validated", error);
                None
            }
        },
    );
    let mut restored = fallback;
    if let Some(placement) = usable {
        let normal = placement.normal;
        let result = window
            .set_position(PhysicalPosition::new(normal.x, normal.y))
            .and_then(|()| window.set_size(PhysicalSize::new(normal.width, normal.height)));
        if let Err(error) = result {
            warn("window placement could not be restored", error);
            if let Some(default) = fallback {
                let _ =
                    window.set_position(PhysicalPosition::new(default.normal.x, default.normal.y));
                let _ = window.set_size(PhysicalSize::new(
                    default.normal.width,
                    default.normal.height,
                ));
            }
        } else {
            restored = Some(Placement {
                normal,
                maximized: cfg!(target_os = "windows") && placement.maximized,
            });
            if cfg!(target_os = "windows") && placement.maximized {
                if let Err(error) = window.maximize() {
                    warn("maximized window state could not be restored", error);
                }
            }
        }
    }
    set_state(state, restored);
    logging::info("window placement restored", serde_json::json!({
        "window": window.label(), "placement": restored,
    }));
}

pub(crate) fn place_preview(
    window: &Window<Wry>,
    fallback: NormalRectangle,
    maximize_fallback: bool,
    state: &PlacementState,
) -> Result<(), String> {
    let saved = state_value(state).and_then(|placement| {
        match usable(window, placement.normal) {
            Ok(true) => Some(placement),
            Ok(false) => None,
            Err(error) => {
                warn("Preview window placement could not be validated", error);
                None
            }
        }
    });
    let placement = saved.unwrap_or(Placement {
        normal: fallback,
        maximized: maximize_fallback,
    });
    window
        .set_position(PhysicalPosition::new(
            placement.normal.x,
            placement.normal.y,
        ))
        .and_then(|()| {
            window.set_size(PhysicalSize::new(
                placement.normal.width,
                placement.normal.height,
            ))
        })
        .and_then(|()| {
            if placement.maximized {
                window.maximize()
            } else {
                Ok(())
            }
        })
        .map_err(|error| error.to_string())?;
    set_state(state, Some(placement));
    logging::info("window placement restored", serde_json::json!({
        "window": window.label(), "placement": placement, "saved": saved.is_some(),
    }));
    Ok(())
}

pub(crate) fn capture(window: &Window<Wry>, state: &PlacementState) {
    capture_with_mode(window, state, cfg!(target_os = "windows"));
}

pub(crate) fn capture_preview(window: &Window<Wry>, state: &PlacementState) {
    capture_with_mode(window, state, true);
}

/// The pure decision behind a placement capture: given what was observed,
/// which `ClosingState` should be recorded? `fullscreen_reported` is the
/// platform's own answer; `presentation_registered` is OneCopy's own
/// fullscreen registry (`fullscreen::is_fullscreen`), consulted in addition
/// because macOS reports OneCopy's raised borderless fullscreen as an ordinary
/// window, and Main's frame in Comparison must never become its saved
/// placement. Isolated from any live `Window` so the decision itself is
/// directly testable.
pub fn closing_state_for(
    minimized: bool,
    fullscreen_reported: bool,
    presentation_registered: bool,
    maximized: bool,
    rectangle: NormalRectangle,
) -> ClosingState {
    if minimized || fullscreen_reported || presentation_registered {
        return ClosingState::Transient;
    }
    if maximized {
        return ClosingState::Maximized;
    }
    ClosingState::Normal(rectangle)
}

fn capture_with_mode(
    window: &Window<Wry>,
    state: &PlacementState,
    remember_maximized: bool,
) {
    let closing = (|| -> tauri::Result<ClosingState> {
        let minimized = window.is_minimized()?;
        let fullscreen_reported = window.is_fullscreen()?;
        let presentation_registered = fullscreen::is_fullscreen(window.label());
        let maximized = is_maximized(window)?;
        let rectangle = current_rectangle(window)?;
        logging::info("window placement sampled", serde_json::json!({
            "window": window.label(), "rectangle": rectangle,
            "minimized": minimized, "fullscreen": fullscreen_reported,
            "presentationRegistered": presentation_registered, "maximized": maximized,
        }));
        Ok(closing_state_for(
            minimized,
            fullscreen_reported,
            presentation_registered,
            maximized,
            rectangle,
        ))
    })();
    match closing {
        Ok(closing) => {
            set_state(
                state,
                placement_after_close(state_value(state), closing, remember_maximized),
            );
        }
        Err(error) => warn("window placement could not be captured", error),
    }
}

/// Placement is sampled only when a window closes and when the app exits,
/// never from move or resize events (window-conventions, "Placement
/// implementation"): all native facts are read together at that moment. A
/// window closed by `destroy()` sends no close request, so Preview closes
/// through `close_preview_window`, which samples first. Accepted under the
/// convention: on Windows, restoring, resizing, maximizing and closing loses
/// that resize, since only the maximized state is observed at close.
pub(crate) fn on_window_event(
    window: &Window<Wry>,
    event: &WindowEvent,
    main_state: &PlacementState,
    preview_state: &PlacementState,
    records_state: &PlacementState,
) {
    if let WindowEvent::CloseRequested { .. } = event {
        match window.label() {
            "main" => capture(window, main_state),
            "preview" => capture_preview(window, preview_state),
            crate::records_window::LABEL => capture(window, records_state),
            _ => {}
        }
    }
}

pub(crate) fn save(state: &PlacementState) {
    let Some(placement) = state_value(state) else {
        return;
    };
    let result = serde_json::to_value(placement)
        .map_err(|error| error.to_string())
        .and_then(|value| storage::save_window_state(&crate::paths::data_root()?, &value));
    if let Err(error) = result {
        warn("window placement could not be saved", error);
    } else {
        logging::info("window placement saved", serde_json::json!({
            "window": "main", "placement": placement,
        }));
    }
}

pub(crate) fn save_records(state: &PlacementState) {
    let Some(placement) = state_value(state) else {
        return;
    };
    let result = serde_json::to_value(placement)
        .map_err(|error| error.to_string())
        .and_then(|value| storage::save_records_window_state(&crate::paths::data_root()?, &value));
    if let Err(error) = result {
        warn("Records window placement could not be saved", error);
    } else {
        logging::info("window placement saved", serde_json::json!({
            "window": crate::records_window::LABEL, "placement": placement,
        }));
    }
}

pub(crate) fn save_preview(state: &PlacementState) {
    let Some(placement) = state_value(state) else {
        return;
    };
    let result = serde_json::to_value(placement)
        .map_err(|error| error.to_string())
        .and_then(|value| storage::save_preview_window_state(&crate::paths::data_root()?, &value));
    if let Err(error) = result {
        warn("Preview window placement could not be saved", error);
    } else {
        logging::info("window placement saved", serde_json::json!({
            "window": "preview", "placement": placement,
        }));
    }
}

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, PhysicalPosition, PhysicalSize, Window, WindowEvent, Wry};

use crate::{logging, paths, storage};

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

pub(crate) type PlacementState = Arc<Mutex<Option<Placement>>>;

pub(crate) fn new_state() -> PlacementState {
    Arc::new(Mutex::new(None))
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

fn usable(window: &Window<Wry>, rectangle: NormalRectangle) -> tauri::Result<bool> {
    if rectangle.width == 0 || rectangle.height == 0 {
        return Ok(false);
    }
    let left = i64::from(rectangle.x);
    let top = i64::from(rectangle.y);
    let right = left + i64::from(rectangle.width);
    let bottom = top + i64::from(rectangle.height);
    Ok(window.available_monitors()?.iter().any(|monitor| {
        let area = monitor.work_area();
        let area_left = i64::from(area.position.x);
        let area_top = i64::from(area.position.y);
        left < area_left + i64::from(area.size.width)
            && right > area_left
            && top < area_top + i64::from(area.size.height)
            && bottom > area_top
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

pub(crate) fn restore(app: &AppHandle, window: &Window<Wry>, state: &PlacementState) {
    let fallback = current_rectangle(window).ok().map(|normal| Placement {
        normal,
        maximized: false,
    });
    let saved = paths::data_root(app)
        .and_then(|root| storage::read_window_state_for_setup(&root))
        .map(|value| value.and_then(|value| serde_json::from_value(value).ok()));
    let saved = match saved {
        Ok(saved) => saved,
        Err(error) => {
            warn("window placement could not be loaded", error);
            None
        }
    };
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
}

pub(crate) fn capture(window: &Window<Wry>, state: &PlacementState) {
    let closing = (|| -> tauri::Result<ClosingState> {
        if window.is_minimized()? || window.is_fullscreen()? {
            return Ok(ClosingState::Transient);
        }
        if window.is_maximized()? {
            return Ok(ClosingState::Maximized);
        }
        Ok(ClosingState::Normal(current_rectangle(window)?))
    })();
    match closing {
        Ok(closing) => {
            set_state(
                state,
                placement_after_close(state_value(state), closing, cfg!(target_os = "windows")),
            );
        }
        Err(error) => warn("window placement could not be captured", error),
    }
}

pub(crate) fn on_window_event(window: &Window<Wry>, event: &WindowEvent, state: &PlacementState) {
    if window.label() == "main" && matches!(event, WindowEvent::CloseRequested { .. }) {
        capture(window, state);
    }
}

pub(crate) fn save(app: &AppHandle, state: &PlacementState) {
    let Some(placement) = state_value(state) else {
        return;
    };
    let result = serde_json::to_value(placement)
        .map_err(|error| error.to_string())
        .and_then(|value| storage::save_window_state(app, &value));
    if let Err(error) = result {
        warn("window placement could not be saved", error);
    }
}

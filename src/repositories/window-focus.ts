import { invoke } from "@tauri-apps/api/core";

/** Focuses a window only while OneCopy is the active app. On macOS a focus
 * activates the app, so work that finishes after the user switched away
 * would bring OneCopy in front of the other app; Rust reads activation as
 * the call lands (`fullscreen::focus_while_active`). */
export function focusWhileActive(label: string): Promise<void> {
  return invoke<void>("focus_window_while_active", { label });
}

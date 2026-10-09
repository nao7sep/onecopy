import { getCurrentWindow } from "@tauri-apps/api/window";
import { log, toErrorFields } from "./logging";

// The focus ring quiets while the window is inactive (interface-styling
// conventions). Inside a webview the page's own focus is not a reliable signal
// for that, so the native window's focus events decide, marked on the root
// where App.css reads it.

export function applyWindowActivity(
  root: Pick<Element, "toggleAttribute">,
  active: boolean,
): void {
  root.toggleAttribute("data-window-inactive", !active);
}

/** Follows the native window's focus for the life of the page. A failed
 * subscription leaves the ring at its active strength, which is only less
 * quiet, never missing. */
export function installWindowActivity(root: Element = document.documentElement): void {
  try {
    void getCurrentWindow()
      .onFocusChanged(({ payload }) => applyWindowActivity(root, payload))
      .catch((error) => log.warn("window focus listener failed", toErrorFields(error)));
  } catch (error) {
    log.warn("window focus listener failed", toErrorFields(error));
  }
}

// Theme application: config's `theme` preference ("system" | "light" |
// "dark", default system) resolves against the OS preference and lands as
// the `.dark` class on the root element — the switch every semantic token in
// App.css keys off. The shared window-appearance bootstrap owns initialization
// and saved-preference updates for every webview.

import { getCurrentWindow } from "@tauri-apps/api/window";
import { log, toErrorFields } from "../repositories";

const prefersDark = () =>
  typeof window !== "undefined" &&
  window.matchMedia("(prefers-color-scheme: dark)").matches;

let currentPref: unknown = "system";
let nativeThemeUpdates = Promise.resolve();

// The old seeded preference duplicated App.css's built-in stack. Treat that
// exact historical value as the default so existing installs see the same
// short, blank preference as new ones; arbitrary user stacks remain verbatim.
const LEGACY_DEFAULT_UI_FONT =
  'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif';

export function resolveDark(pref: unknown, systemDark: boolean): boolean {
  return pref === "dark" || (pref !== "light" && systemDark);
}

export function applyTheme(pref: unknown): void {
  currentPref = pref;
  const dark = resolveDark(pref, prefersDark());
  document.documentElement.classList.toggle("dark", dark);

  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    const nativeTheme = pref === "light" || pref === "dark" ? pref : null;
    nativeThemeUpdates = nativeThemeUpdates
      .then(() => getCurrentWindow().setTheme(nativeTheme))
      .catch((error) => {
        log.warn("native window theme update failed", toErrorFields(error));
      });
  }
}

export function normalizeUiFontPreference(family: unknown): string {
  if (typeof family !== "string") return "";
  return family.trim() === LEGACY_DEFAULT_UI_FONT ? "" : family;
}

/** Applies the configured UI font by setting the one `--font-ui` variable —
 * the value every surface inherits through App.css's body rule. Stored
 * verbatim and handed to CSS, which resolves the stack and falls back on its
 * own (app-chrome conventions); an empty or non-string value clears the
 * override so the stylesheet default rules. */
export function applyUiFont(family: unknown): void {
  const value = normalizeUiFontPreference(family).trim();
  if (value === "") {
    document.documentElement.style.removeProperty("--font-ui");
  } else {
    document.documentElement.style.setProperty("--font-ui", value);
  }
}

/** Re-applies on OS theme changes while the preference is "system". */
export function watchSystemTheme(): () => void {
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  const changed = () => {
    if (currentPref !== "light" && currentPref !== "dark") applyTheme(currentPref);
  };
  media.addEventListener("change", changed);
  return () => media.removeEventListener("change", changed);
}

// The configured UI font, applied to every webview by the shared
// window-appearance bootstrap. (The theme is not applied here: the Rust core
// sets each window's theme natively and App.css follows it through
// prefers-color-scheme.)

// The old seeded preference duplicated App.css's built-in stack. Treat that
// exact historical value as the default so existing installs see the same
// short, blank preference as new ones; arbitrary user stacks remain verbatim.
const LEGACY_DEFAULT_UI_FONT =
  'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif';

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

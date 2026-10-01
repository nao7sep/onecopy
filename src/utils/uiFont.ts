// The configured UI font, applied to every webview by the shared
// window-appearance bootstrap. (The theme is not applied here: the Rust core
// sets each window's theme natively and App.css follows it through
// prefers-color-scheme.)

export function normalizeUiFontPreference(family: unknown): string {
  return typeof family === "string" ? family : "";
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

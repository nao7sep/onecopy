// Shortcut display + detection helpers (keyboard-shortcut-conventions): bind
// both Cmd and Ctrl, DISPLAY the running platform's single word, spell keys in
// full, `+` joiner, tight `/` only for alternatives sharing a modifier.

// navigator.platform is deprecated but still reliable in all current engines
// including Tauri's webview.
const isApplePlatform = /Mac|iPhone|iPad|iPod/.test(
  typeof navigator === "undefined" ? "" : navigator.platform || navigator.userAgent,
);

/** The platform's command-modifier word: Cmd on macOS, Ctrl elsewhere. */
export function primaryModWord(): string {
  return isApplePlatform ? "Cmd" : "Ctrl";
}

/** What the file manager is called here. Naming the wrong one is a small lie
 * that makes the whole surface feel foreign, so it follows the same
 * display-the-running-platform rule the modifier word does. */
export function fileManagerWord(): string {
  return isApplePlatform ? "Finder" : "Explorer";
}

/**
 * BOTH Cmd and Ctrl fire the command on every platform (the conventions'
 * cross-machine muscle-memory rule); only the DISPLAY word is platform-bound.
 * The one shared detector — zoom and every future chord import this.
 */
export function hasMod(event: KeyboardEvent): boolean {
  return event.metaKey || event.ctrlKey;
}

/** Cmd+Slash, with bare Question as the conventional alias. */
export function isHelpShortcut(event: KeyboardEvent): boolean {
  if (hasMod(event) && event.key === "/") return true;
  return event.key === "?" && !hasMod(event) && !event.altKey;
}

/** Cmd+Comma — the fleet's Settings chord (modal-dialog conventions). */
export function isSettingsShortcut(event: KeyboardEvent): boolean {
  return hasMod(event) && event.key === ",";
}

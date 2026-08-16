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
 * Alt is excluded so Windows AltGr — delivered as Ctrl+Alt by the webview —
 * keeps typing characters instead of firing accelerators (the Chromium /
 * VS Code rule). The one shared detector — zoom and every future chord
 * import this.
 */
export function hasMod(event: KeyboardEvent): boolean {
  return (event.metaKey || event.ctrlKey) && !event.altKey;
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

/** INPUT types that consume printable keys. A checkbox, radio, range, button
 * or file picker does not, so a chord must NOT stand down over one (the
 * keyboard-shortcut-conventions name these explicitly). The empty string is
 * `<input>` with no type attribute, which is a text field. */
const TEXT_INPUT_TYPES = new Set([
  "",
  "email",
  "number",
  "password",
  "search",
  "tel",
  "text",
  "url",
]);

function consumesPrintableKeys(node: Element): boolean {
  if (node instanceof HTMLElement && node.isContentEditable) return true;
  const tag = node.tagName.toUpperCase();
  if (tag === "TEXTAREA") return true;
  if (tag !== "INPUT") return false;
  return TEXT_INPUT_TYPES.has((node as HTMLInputElement).type.toLowerCase());
}

/**
 * Is the user typing into this event's target? The ONE definition for the
 * whole app (keyboard-shortcut-conventions: "'Editable' is one predicate per
 * app") — three inline copies had already drifted apart, one of them silently
 * dropping the contenteditable case.
 *
 * The walk up `parentElement` is load-bearing rather than decorative: a rich
 * editor's real event target is a descendant of the contenteditable host, so
 * a tagName-only test sees a plain DIV and lets every chord through. OneCopy
 * has no such surface today — all its text entry is INPUT/TEXTAREA — so the
 * walk costs nothing now and is what keeps the rule true when one appears.
 */
export function isEditableTarget(target: EventTarget | null): boolean {
  let node = target instanceof Element ? target : null;
  while (node !== null) {
    if (consumesPrintableKeys(node)) return true;
    node = node.parentElement;
  }
  return false;
}

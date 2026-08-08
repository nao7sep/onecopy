// Shortcut display + detection helpers (keyboard-shortcut-conventions): bind
// both Cmd and Ctrl, DISPLAY the running platform's single word, spell keys in
// full, `+` joiner, tight `/` only for alternatives sharing a modifier.

const isApplePlatform = /Mac|iPhone|iPad|iPod/.test(
  typeof navigator === "undefined" ? "" : navigator.platform || navigator.userAgent,
);

/** The platform's command-modifier word: Cmd on macOS, Ctrl elsewhere. */
export function primaryModWord(): string {
  return isApplePlatform ? "Cmd" : "Ctrl";
}

function hasMod(event: KeyboardEvent): boolean {
  return isApplePlatform ? event.metaKey : event.ctrlKey;
}

/** Cmd+Slash, with bare Question as the conventional alias. */
export function isHelpShortcut(event: KeyboardEvent): boolean {
  if (hasMod(event) && event.key === "/") return true;
  return event.key === "?" && !hasMod(event) && !event.altKey;
}

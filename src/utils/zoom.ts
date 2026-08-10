// The one shared command-modifier detector (both Cmd and Ctrl on every
// platform). Local duplication here is exactly where the both-modifiers fix
// was forgotten once, so zoom imports it instead.
import { hasMod } from "./shortcuts";

// Zoom utilities: discrete zoom level list and keyboard shortcut detection.
//
// --- Zoom levels ---
//
// ZOOM_LEVELS is the ordered list of supported zoom multipliers (1.0 = 100%).
// Steps follow a roughly 1.2× ratio with human-friendly rounded values.
// The 1.2 base comes from Electron's zoom formula `scale = 1.2 ^ level`,
// which VS Code also uses (src/vs/platform/window/common/window.ts):
//   under 100%: last digit is 0 (e.g. 50, 60, 70 ...)
//   100% exactly: the default
//   over  100%: last digit is 0 (e.g. 120, 140, 170 ...)
// Use stepZoomIn / stepZoomOut to move through this list from a current value.
// Use ZOOM_DEFAULT (1.0) as the reset target.
//
// --- Keyboard shortcuts ---
//
// isZoomIn / isZoomOut / isZoomReset accept a KeyboardEvent and return true
// when the event matches the corresponding zoom shortcut. All three require
// the command modifier — both Cmd and Ctrl fire on every platform; only the
// displayed word is platform-bound.
//
// --- Keyboard layout notes ---
//
// "+" is not a bare key on any major layout; it always requires Shift. The base
// key that produces "+" varies across layouts:
//   US / UK / French AZERTY  →  Shift + =  (physical = key, left of Backspace)
//   Japanese JIS              →  Shift + ;  (physical ; key, near Enter)
//   German QWERTZ             →  Shift + 1  (not covered here; numpad serves them)
//
// "-" is a bare key (no Shift) on US, UK, German, and Japanese layouts.
// French AZERTY users need Shift for "-", so they are not covered by zoom out;
// this is an accepted gap.
//
// Using event.key (the produced character) rather than event.code (physical
// position) means each case is handled automatically:
//   "="  → physical = key without Shift  (US/UK/French)
//   "+"  → = key + Shift, ; key + Shift, Numpad+ (with or without Shift),
//           or a bare "+" from a programmable / remapped keyboard
//   ";"  → physical ; key without Shift  (Japanese JIS)
//          Claude Code and Codex use this same key for zoom in on JIS keyboards.
//   "-"  → physical - key or Numpad-  (event.key is "-" for both)
//   "0"  → main row 0 or Numpad 0     (event.key is "0" for both)

// Ordered list of supported zoom multipliers (1.0 = 100%).
// Percentages: 50, 60, 70, 80, 90, 100, 120, 140, 170, 200, 240, 290, 350, 420, 500
//
// Steps follow a roughly 1.2× ratio, matching the Electron / VS Code zoom formula
// `scale = 1.2 ^ level` (VS Code: src/vs/platform/window/common/window.ts,
// Electron docs: https://www.electronjs.org/docs/latest/api/web-contents#contentssetZoomLevellevel).
// Values are hand-picked for readability: last digit 0 below 100, last digit 0 above 100.
export const ZOOM_LEVELS = [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.2, 1.4, 1.7, 2.0, 2.4, 2.9, 3.5, 4.2, 5.0];

export const ZOOM_DEFAULT = 1.0;
export const ZOOM_MIN = ZOOM_LEVELS[0];
export const ZOOM_MAX = ZOOM_LEVELS[ZOOM_LEVELS.length - 1];

// Returns the nearest zoom level in ZOOM_LEVELS to the given value.
// Used to snap an arbitrary stored value onto the discrete list.
function nearest(value: number): number {
  return ZOOM_LEVELS.reduce((a, b) =>
    Math.abs(b - value) < Math.abs(a - value) ? b : a,
  );
}

// Returns the next zoom level above the current value, or the current maximum.
export function stepZoomIn(current: number): number {
  const snapped = nearest(current);
  const idx = ZOOM_LEVELS.indexOf(snapped);
  return ZOOM_LEVELS[Math.min(idx + 1, ZOOM_LEVELS.length - 1)];
}

// Returns the next zoom level below the current value, or the current minimum.
export function stepZoomOut(current: number): number {
  const snapped = nearest(current);
  const idx = ZOOM_LEVELS.indexOf(snapped);
  return ZOOM_LEVELS[Math.max(idx - 1, 0)];
}


// Returns true when the event is a zoom-in shortcut (primary modifier + zoom key).
// Recognized zoom-in keys: "=", "+", ";"
export function isZoomIn(event: KeyboardEvent): boolean {
  if (!hasMod(event)) return false;
  return event.key === "=" || event.key === "+" || event.key === ";";
}

// Returns true when the event is a zoom-out shortcut (primary modifier + "-").
export function isZoomOut(event: KeyboardEvent): boolean {
  if (!hasMod(event)) return false;
  return event.key === "-";
}

// Returns true when the event is a zoom-reset shortcut (primary modifier + "0").
export function isZoomReset(event: KeyboardEvent): boolean {
  if (!hasMod(event)) return false;
  return event.key === "0";
}

// Every window call the app makes must be a capability it was granted.
//
// This exists because of a real, silent failure: `PreviewWindow` called
// `setFullscreen()` while `capabilities/default.json` granted only
// `core:window:allow-is-fullscreen`. Tauri rejected the call, a
// `.catch(() => {})` swallowed the rejection, and the preview footer went on
// advertising "F: fullscreen" — a key that did nothing, through a release
// build, past every green suite, until a person pressed it.
//
// Nothing else can catch this: the call compiles, the permission is data in a
// JSON file, and the failure is a runtime rejection on a machine nobody
// automated. So the pairing is pinned here instead.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const SOURCES = [
  "src/App.tsx",
  "src/state/preview-store.ts",
  "src/state/comparison-store.ts",
  "src/windows/PreviewWindow.tsx",
  "src/windows/ComparisonWindow.tsx",
  "src/windows/IdentifyWindow.tsx",
  "src/utils/windowSizing.ts",
].map((path) => readFileSync(path, "utf8"));
const ALL_SOURCE = SOURCES.join("\n");

const capabilities = JSON.parse(
  readFileSync("src-tauri/capabilities/default.json", "utf8"),
) as { permissions: string[] };

/** Window methods the app calls, and the permission each one needs. Kebab-case
 * of the method name is Tauri's own convention, so the mapping is mechanical —
 * what matters is that a method appearing in the source has its row here. */
const NEEDS: Record<string, string> = {
  "setFullscreen(": "core:window:allow-set-fullscreen",
  "setAlwaysOnTop(": "core:window:allow-set-always-on-top",
  "isFullscreen(": "core:window:allow-is-fullscreen",
  "setFocus(": "core:window:allow-set-focus",
  "setMinSize(": "core:window:allow-set-min-size",
  "setSize(": "core:window:allow-set-size",
  "setPosition(": "core:window:allow-set-position",
  "setTitle(": "core:window:allow-set-title",
  "setTheme(": "core:window:allow-set-theme",
  "availableMonitors(": "core:window:allow-available-monitors",
  "outerPosition(": "core:window:allow-outer-position",
  "innerSize(": "core:window:allow-inner-size",
  "currentMonitor(": "core:window:allow-current-monitor",
  ".show(": "core:window:allow-show",
  ".hide(": "core:window:allow-hide",
  ".close(": "core:window:allow-close",
  ".destroy(": "core:window:allow-destroy",
  "setZoom(": "core:webview:allow-set-webview-zoom",
};

describe("window calls and granted capabilities", () => {
  const used = Object.entries(NEEDS).filter(([call]) => ALL_SOURCE.includes(call));

  it("finds the window calls it is meant to be checking", () => {
    // A guard on the guard: if a refactor renames these call sites, the loop
    // below would pass by checking nothing at all.
    expect(used.length).toBeGreaterThan(6);
  });

  it.each(used)("%s is granted (%s)", (_call, permission) => {
    expect(capabilities.permissions).toContain(permission);
  });

  it("grants fullscreen in BOTH directions, the pair that broke", () => {
    // Reading the state without being able to change it is the exact shape of
    // the original defect.
    expect(capabilities.permissions).toContain("core:window:allow-is-fullscreen");
    expect(capabilities.permissions).toContain("core:window:allow-set-fullscreen");
  });
});

describe("failed window calls are reported, never swallowed", () => {
  it("uses no bare catch around a Tauri call", () => {
    // `.catch(() => {})` is what turned a missing permission into an
    // invisible no-op. These calls stay best-effort — a window the user just
    // closed must not throw — but the reason reaches the log.
    for (const source of SOURCES) {
      expect(source).not.toContain("catch(() => {})");
    }
  });
});

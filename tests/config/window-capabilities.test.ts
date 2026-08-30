// Every window call the app makes must be a capability it was granted.
//
// This exists because window capabilities are runtime data: a call compiles
// even when its permission is absent. OneCopy's true fullscreen deliberately
// uses its app command rather than Tauri's native Spaces fullscreen.
//
// Nothing else can catch this: the call compiles, the permission is data in a
// JSON file, and the failure is a runtime rejection on a machine nobody
// automated. So the pairing is pinned here instead.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const SOURCES = [
  "src/App.tsx",
  "src/hooks/useMainWindowLifecycle.ts",
  "src/state/preview-store.ts",
  "src/state/comparison-store.ts",
  "src/workflows/quick-view.ts",
  "src/workflows/viewer-window.ts",
  "src/windows/PreviewWindow.tsx",
  "src/windows/ViewerWindow.tsx",
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
  "setAlwaysOnTop(": "core:window:allow-set-always-on-top",
  "setFocus(": "core:window:allow-set-focus",
  "setMinSize(": "core:window:allow-set-min-size",
  "setSize(": "core:window:allow-set-size",
  "setPosition(": "core:window:allow-set-position",
  "setTitle(": "core:window:allow-set-title",
  "setTheme(": "core:window:allow-set-theme",
  "availableMonitors(": "core:window:allow-available-monitors",
  "outerPosition(": "core:window:allow-outer-position",
  "innerSize(": "core:window:allow-inner-size",
  "isMaximized(": "core:window:allow-is-maximized",
  ".maximize(": "core:window:allow-maximize",
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

  it("does not grant or call native Spaces fullscreen", () => {
    expect(ALL_SOURCE).not.toContain("setFullscreen(");
    expect(capabilities.permissions).not.toContain("core:window:allow-set-fullscreen");
    expect(capabilities.permissions).not.toContain("core:window:allow-is-fullscreen");
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

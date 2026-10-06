// Every window call the app makes must be a capability it was granted.
//
// This exists because window capabilities are runtime data: a call compiles
// even when its permission is absent. Fullscreen is OneCopy's own app command
// (src-tauri/src/fullscreen.rs), never Tauri's window fullscreen, which on
// macOS is Spaces fullscreen.
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
  "src/workflows/fullscreen-view.ts",
  "src/workflows/fullscreen-view-window.ts",
  "src/windows/PreviewWindow.tsx",
  "src/windows/FullscreenViewWindow.tsx",
  "src/windows/ComparisonWindow.tsx",
  "src/windows/IdentifyWindow.tsx",
  "src/windows/RecordsWindow.tsx",
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
  "isMaximized(": "core:window:allow-is-maximized",
  ".maximize(": "core:window:allow-maximize",
  ".unmaximize(": "core:window:allow-unmaximize",
  "isMinimized(": "core:window:allow-is-minimized",
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

  it("keeps native fullscreen setters behind the platform-aware app command", () => {
    // App-owned setters may have the same name; webviews must not receive
    // permission to invoke Tauri's macOS Spaces setter directly.
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

describe("durable window-state boundary", () => {
  const core = readFileSync("src-tauri/src/lib.rs", "utf8");
  const placement = readFileSync("src-tauri/src/window_placement.rs", "utf8");
  const cargoManifest = readFileSync("src-tauri/Cargo.toml", "utf8");
  const tauriConfig = JSON.parse(
    readFileSync("src-tauri/tauri.conf.json", "utf8"),
  ) as { app: { windows: Array<{ visible?: boolean }> } };

  it("restores Main's atomic native record before showing it", () => {
    expect(core).toContain("window_placement::restore(");
    expect(core.indexOf("window_placement::restore(")).toBeLessThan(core.indexOf("window.show()"));
    expect(placement).toContain('"main" => capture(window, main_state)');
    expect(placement).toContain('"preview" => capture_preview(window, preview_state)');
    expect(placement).not.toMatch(/debounce|prev_[xy]/i);
    expect(cargoManifest).not.toContain("tauri-plugin-window-state");
  });

  it("keeps live Moved/Resized handling in memory, not on disk", () => {
    // Moved/Resized may keep the in-memory normal rectangle current (so a
    // resize just before close is not lost), but only CloseRequested may
    // write to disk. A save on every move would reintroduce the debounced
    // polling that tauri-plugin-window-state was replaced to avoid.
    const movedResizedArm = placement.slice(
      placement.indexOf("WindowEvent::Moved"),
      placement.indexOf("_ => {}", placement.indexOf("WindowEvent::Moved")),
    );
    expect(movedResizedArm).not.toMatch(/storage::save|\bsave\(|\bsave_preview\(/);
  });

  it("creates Main hidden only for native setup", () => {
    expect(tauriConfig.app.windows[0]?.visible).toBe(false);
  });
});

describe("the Records window", () => {
  const core = readFileSync("src-tauri/src/lib.rs", "utf8");
  const placement = readFileSync("src-tauri/src/window_placement.rs", "utf8");
  const records = readFileSync("src-tauri/src/records_window.rs", "utf8");
  const capabilityWindows = (
    JSON.parse(readFileSync("src-tauri/capabilities/default.json", "utf8")) as { windows: string[] }
  ).windows;

  it("is one window: opening it again brings the open one forward", () => {
    expect(records).toContain('pub const LABEL: &str = "records";');
    const open = records.slice(records.indexOf("pub fn open("));
    expect(open.indexOf("get_webview_window(LABEL)")).toBeLessThan(open.indexOf("WebviewWindowBuilder::new("));
    expect(capabilityWindows).toContain("records");
  });

  it("is placed while hidden, before it is shown", () => {
    const open = records.slice(records.indexOf("pub fn open("));
    expect(open).toContain(".visible(false)");
    expect(open.indexOf("place_records(")).toBeLessThan(open.lastIndexOf("bring_forward(&window)"));
  });

  it("keeps its own placement, captured at close and at exit and saved once at exit", () => {
    expect(placement).toContain("crate::records_window::LABEL => capture(window, records_state)");
    expect(core).toContain("window_placement::load_records(");
    expect(core).toMatch(/get_webview_window\(records_window::LABEL\)[\s\S]*?window_placement::capture\(/);
    expect(core).toContain("window_placement::save_records(");
  });

  it("does not stop a Dock click from bringing Main back", () => {
    const reopen = core.slice(core.indexOf("tauri::RunEvent::Reopen"));
    expect(reopen).toContain('get_webview_window("main")');
    expect(reopen).toContain(".unminimize()");
  });
});

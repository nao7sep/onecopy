// Vitest global setup.
//
// The default `node` environment has no `navigator`; utilities that read the
// platform at module load must never throw under `node`, so a bare stub is
// installed here. Tests that need a specific platform stub it themselves.

import { afterEach, vi } from "vitest";

// Several stores reach each other through fire-and-forget dynamic imports
// (`void import("./preview-store").then(...)`), which is correct in the app —
// nothing awaits an anchor notification. In tests those imports can still be
// resolving when the file finishes, and loading a module after the environment
// is torn down surfaces as an unhandled EnvironmentTeardownError: all tests
// pass, but runs are intermittently noisy.
//
// A module load is a macrotask, so draining microtasks is not enough — and the
// real timer is captured HERE, before any spec installs fake ones, so a suite
// using vi.useFakeTimers() still gets a genuine tick.
const realSetTimeout = globalThis.setTimeout;
afterEach(async () => {
  await new Promise<void>((resolve) => realSetTimeout(resolve, 0));
});

if (typeof globalThis.navigator === "undefined") {
  vi.stubGlobal("navigator", { platform: "", userAgent: "" });
}

// Tauri is faked for the whole suite from here, because this is the only place
// a `vi.mock` reaches every spec file — registering them per-spec would drift.
// The doubles and their controls live in tests/mocks/tauri.ts; each factory
// pulls from that one module so a spec and its mock share the same state.
// Specs opt in by importing the controls, not by re-registering the mock.

vi.mock("@tauri-apps/api/core", async () => {
  const m = await import("./mocks/tauri");
  return { invoke: m.invoke, convertFileSrc: m.convertFileSrc };
});

vi.mock("@tauri-apps/api/event", async () => {
  const m = await import("./mocks/tauri");
  return { listen: m.listen, emit: m.emit };
});

vi.mock("@tauri-apps/api/window", async () => {
  const m = await import("./mocks/tauri");
  return {
    getCurrentWindow: m.getCurrentWindow,
    availableMonitors: m.availableMonitors,
    LogicalSize: m.LogicalSize,
  };
});

vi.mock("@tauri-apps/api/webview", async () => {
  const m = await import("./mocks/tauri");
  return { getCurrentWebview: m.getCurrentWebview };
});

vi.mock("@tauri-apps/api/webviewWindow", async () => {
  const m = await import("./mocks/tauri");
  return { WebviewWindow: m.WebviewWindow };
});

vi.mock("@tauri-apps/plugin-dialog", async () => {
  const m = await import("./mocks/tauri");
  return { open: m.openDialog };
});

vi.mock("@tauri-apps/plugin-opener", async () => {
  const m = await import("./mocks/tauri");
  return { openPath: m.openPath, openUrl: m.openUrl };
});

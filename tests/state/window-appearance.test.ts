// @vitest-environment happy-dom

import { afterEach, beforeEach, expect, it, vi } from "vitest";
// Resolve the shared SDK doubles once; resetting app modules below must not
// create a second IPC registry distinct from these test controls.
import "@tauri-apps/api/core";
import "@tauri-apps/api/event";
import "@tauri-apps/api/window";
import { fireEvent, invokeCalls, listenerCount, mockCommands, resetTauriMocks, setTheme } from "../mocks/tauri";

let systemDark = true;
let systemChanges: Set<() => void>;

beforeEach(() => {
  vi.resetModules();
  resetTauriMocks();
  systemDark = true;
  systemChanges = new Set();
  vi.spyOn(window, "matchMedia").mockImplementation(() => ({
    get matches() { return systemDark; },
    addEventListener: (_: string, handler: () => void) => systemChanges.add(handler),
    removeEventListener: (_: string, handler: () => void) => systemChanges.delete(handler),
  }) as unknown as MediaQueryList);
  Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
  mockCommands({ record_interface_failure: () => null, log_event: () => null });
});

afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });

it("initializes theme and font without Main bootstrap, then follows saved and System changes", async () => {
  let preferences = { theme: "system", uiFontFamily: "Iosevka" };
  mockCommands({ appearance_preferences: () => preferences });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  await installWindowAppearance();
  expect(listenerCount("appearance://changed")).toBe(1);
  expect(document.documentElement.classList.contains("dark")).toBe(true);
  expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("Iosevka");
  expect(invokeCalls.some((call) => call.command === "load_app_data")).toBe(false);

  systemDark = false;
  systemChanges.forEach((handler) => handler());
  expect(document.documentElement.classList.contains("dark")).toBe(false);
  preferences = { theme: "dark", uiFontFamily: "" };
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(setTheme).toHaveBeenLastCalledWith("dark"));
  expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("");
  systemChanges.forEach((handler) => handler());
  expect(document.documentElement.classList.contains("dark")).toBe(true);
  await installWindowAppearance();
  expect(listenerCount("appearance://changed")).toBe(1);
});

it("ignores a delayed startup response after a newer saved preference", async () => {
  let resolveFirst!: (value: unknown) => void;
  mockCommands({ appearance_preferences: () => new Promise((resolve) => { resolveFirst = resolve; }) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const installing = installWindowAppearance();
  await vi.waitFor(() => expect(resolveFirst).toBeDefined());
  mockCommands({ appearance_preferences: () => ({ theme: "light", uiFontFamily: "New font" }) });
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("New font"));
  resolveFirst({ theme: "dark", uiFontFamily: "Old font" });
  await installing;
  expect(document.documentElement.classList.contains("dark")).toBe(false);
  expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("New font");
});

it("contains read failure and leaves the listener ready for a repaired configuration", async () => {
  mockCommands({ appearance_preferences: () => { throw new Error("unreadable config"); } });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  await installWindowAppearance();
  expect(invokeCalls.some((call) => call.command === "record_interface_failure")).toBe(true);
  expect(listenerCount("appearance://changed")).toBe(1);
  mockCommands({ appearance_preferences: () => ({ theme: "light", uiFontFamily: "Recovered font" }) });
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("Recovered font"));
});

it("bounds a stalled preference read so appearance cannot indefinitely hold window startup", async () => {
  vi.useFakeTimers();
  mockCommands({ appearance_preferences: () => new Promise(() => {}) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const installing = installWindowAppearance();
  await vi.advanceTimersByTimeAsync(5_000);
  await installing;
  expect(invokeCalls.some((call) => call.command === "record_interface_failure")).toBe(true);
});

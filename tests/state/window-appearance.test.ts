// @vitest-environment happy-dom

import { afterEach, beforeEach, expect, it, vi } from "vitest";
// Resolve the shared SDK doubles once; resetting app modules below must not
// create a second IPC registry distinct from these test controls.
import "@tauri-apps/api/core";
import "@tauri-apps/api/event";
import "@tauri-apps/api/window";
import { fireEvent, invokeCalls, listenerCount, mockCommands, resetTauriMocks, setTheme } from "../mocks/tauri";

beforeEach(() => {
  vi.resetModules();
  resetTauriMocks();
  Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
  mockCommands({ record_interface_failure: () => null, log_event: () => null });
});

afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });

it("initializes the font without Main bootstrap, then follows saved changes", async () => {
  let preferences = { uiFontFamily: "Iosevka" };
  mockCommands({ appearance_preferences: () => preferences });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const { useWindowPreferencesStore } = await import("../../src/state/window-preferences-store");
  await installWindowAppearance();
  expect(listenerCount("appearance://changed")).toBe(1);
  expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("Iosevka");
  expect(useWindowPreferencesStore.getState().videoTranscriptionEnabled).toBe(true);
  expect(invokeCalls.some((call) => call.command === "load_app_data")).toBe(false);

  preferences = { uiFontFamily: "" };
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe(""));
  await installWindowAppearance();
  expect(listenerCount("appearance://changed")).toBe(1);
});

it("leaves the theme to the native window", async () => {
  mockCommands({ appearance_preferences: () => ({ uiFontFamily: null }) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  await installWindowAppearance();
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(invokeCalls.filter((call) => call.command === "appearance_preferences")).toHaveLength(2));
  expect(setTheme).not.toHaveBeenCalled();
  expect(document.documentElement.classList.contains("dark")).toBe(false);
});

it("projects auxiliary Preview preferences without running Main bootstrap", async () => {
  mockCommands({
    appearance_preferences: () => ({
      uiFontFamily: null,
      enlargeSmallImagesInPreview: false,
      videoTranscriptionEnabled: false,
      audioTranscriptionEnabled: true,
    }),
  });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const { useWindowPreferencesStore } = await import("../../src/state/window-preferences-store");
  await installWindowAppearance();
  expect(useWindowPreferencesStore.getState()).toMatchObject({
    enlargeSmallImagesInPreview: false,
    videoTranscriptionEnabled: false,
    audioTranscriptionEnabled: true,
  });
  expect(invokeCalls.some((call) => call.command === "load_app_data")).toBe(false);
});

it("ignores a delayed startup response after a newer saved preference", async () => {
  let resolveFirst!: (value: unknown) => void;
  mockCommands({ appearance_preferences: () => new Promise((resolve) => { resolveFirst = resolve; }) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const installing = installWindowAppearance();
  await vi.waitFor(() => expect(resolveFirst).toBeDefined());
  mockCommands({ appearance_preferences: () => ({ uiFontFamily: "New font" }) });
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("New font"));
  resolveFirst({ uiFontFamily: "Old font" });
  await installing;
  expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("New font");
});

it("contains read failure and leaves the listener ready for a repaired configuration", async () => {
  mockCommands({ appearance_preferences: () => { throw new Error("unreadable config"); } });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  await installWindowAppearance();
  expect(invokeCalls.some((call) => call.command === "record_interface_failure")).toBe(true);
  expect(listenerCount("appearance://changed")).toBe(1);
  mockCommands({ appearance_preferences: () => ({ uiFontFamily: "Recovered font" }) });
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

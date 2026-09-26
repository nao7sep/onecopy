// @vitest-environment happy-dom

import { afterEach, beforeEach, expect, it, vi } from "vitest";
// Resolve the shared SDK doubles once; resetting app modules below must not
// create a second IPC registry distinct from these test controls.
import "@tauri-apps/api/core";
import "@tauri-apps/api/event";
import "@tauri-apps/api/window";
import { fireEvent, invokeCalls, listenerCount, mockCommands, resetTauriMocks, setTheme, setTitle } from "../mocks/tauri";

beforeEach(() => {
  vi.resetModules();
  resetTauriMocks();
  window.history.pushState(null, "", "/");
  Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
  mockCommands({ record_interface_failure: () => null, log_event: () => null });
});

afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); window.history.pushState(null, "", "/"); });

it("initializes the font without Main bootstrap, then follows saved changes", async () => {
  // The core answers with effective values (storage::read_appearance_preferences).
  const effective = {
    enlargeSmallImagesInPreview: true,
    enlargeSmallImagesInQuickView: true,
    videoTranscriptionEnabled: true,
    audioTranscriptionEnabled: true,
  };
  let preferences = { ...effective, uiFontFamily: "Iosevka" };
  mockCommands({ appearance_preferences: () => preferences });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const { useWindowPreferencesStore } = await import("../../src/state/window-preferences-store");
  await installWindowAppearance();
  expect(listenerCount("appearance://changed")).toBe(1);
  expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("Iosevka");
  expect(useWindowPreferencesStore.getState().videoTranscriptionEnabled).toBe(true);
  expect(invokeCalls.some((call) => call.command === "load_app_data")).toBe(false);

  preferences = { ...effective, uiFontFamily: "" };
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
      enlargeSmallImagesInQuickView: true,
      videoTranscriptionEnabled: false,
      audioTranscriptionEnabled: true,
    }),
  });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const { useWindowPreferencesStore } = await import("../../src/state/window-preferences-store");
  await installWindowAppearance();
  expect(useWindowPreferencesStore.getState()).toMatchObject({
    enlargeSmallImagesInPreview: false,
    // Fullscreen (an auxiliary webview) shares Quick View's own setting, not
    // Preview's — the two are one session (content-presentation.md D3).
    enlargeSmallImagesInQuickView: true,
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

it("sets Main's title on first paint and again after a language change (D-L6)", async () => {
  let language: unknown = "en";
  mockCommands({ appearance_preferences: () => ({ uiFontFamily: null, language }) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  await installWindowAppearance();
  expect(setTitle).toHaveBeenCalledWith("OneCopy");

  language = "ja";
  setTitle.mockClear();
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(setTitle).toHaveBeenCalledWith("OneCopy"));
});

it("titles an auxiliary window from the current language, not fixed English (D-L6)", async () => {
  window.history.pushState(null, "", "/?view=preview");
  let language: unknown = "en";
  mockCommands({ appearance_preferences: () => ({ uiFontFamily: null, language }) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  await installWindowAppearance();
  expect(setTitle).toHaveBeenCalledWith("OneCopy Preview");

  language = "ja";
  setTitle.mockClear();
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(setTitle).toHaveBeenCalledWith("OneCopy プレビュー"));
});

it("leaves the identify flash and comparison-image titles alone (they carry no app sentence)", async () => {
  window.history.pushState(null, "", "/?view=identify&slice=1");
  mockCommands({ appearance_preferences: () => ({ uiFontFamily: null }) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  await installWindowAppearance();
  expect(setTitle).not.toHaveBeenCalled();
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

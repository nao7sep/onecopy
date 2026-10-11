// @vitest-environment happy-dom

import { afterEach, beforeEach, expect, it, vi } from "vitest";
import "@tauri-apps/api/core";
import "@tauri-apps/api/event";
import "@tauri-apps/api/window";
import { fireEvent, invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import type { Language } from "../../src/i18n/languages";

beforeEach(() => {
  vi.resetModules();
  resetTauriMocks();
  mockCommands({ log_event: () => null, record_interface_failure: () => null });
  Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
});
afterEach(() => vi.restoreAllMocks());

async function delayCatalogue(language: Language) {
  const catalogues = await import("../../src/i18n/catalogues");
  const load = catalogues.loadCatalogue;
  let finish!: () => void;
  let entered = false;
  const gate = new Promise<void>((resolve) => { finish = resolve; });
  vi.spyOn(catalogues, "loadCatalogue").mockImplementation(async (requested) => {
    if (requested === language) {
      entered = true;
      await gate;
    }
    return await load(requested);
  });
  return { catalogues, finish, entered: () => entered };
}

it("publishes only the newest language while still caching an obsolete catalogue", async () => {
  const delayed = await delayCatalogue("ja");
  const { useLanguageStore } = await import("../../src/state/language-store");
  const older = useLanguageStore.getState().apply({ language: "ja", systemLanguage: "ja", systemLocale: "ja-JP" });
  await useLanguageStore.getState().apply({ language: "en", systemLanguage: "en", systemLocale: "en-US" });
  delayed.finish();
  await older;
  expect(useLanguageStore.getState()).toMatchObject({ language: "en", systemLanguage: "en", systemLocale: "en-US" });
  expect(delayed.catalogues.catalogueFor("ja")["wizard.language"]).toBe("言語");
});

it("does not publish an older appearance language after a newer refresh completed", async () => {
  const delayed = await delayCatalogue("ja");
  mockCommands({ appearance_preferences: () => ({ language: "ja", uiFontFamily: "Old font" }) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const { useLanguageStore } = await import("../../src/state/language-store");
  const installing = installWindowAppearance();
  await vi.waitFor(() => expect(delayed.entered()).toBe(true));
  mockCommands({ appearance_preferences: () => ({ language: "en", uiFontFamily: "New font" }) });
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("New font"));
  delayed.finish();
  await installing;
  expect(useLanguageStore.getState().language).toBe("en");
  expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("New font");
});

it("invalidates an older catalogue when a newer appearance read is admitted but still pending", async () => {
  const delayed = await delayCatalogue("ja");
  mockCommands({ appearance_preferences: () => ({ language: "en", uiFontFamily: "Initial font" }) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const { useLanguageStore } = await import("../../src/state/language-store");
  await installWindowAppearance();
  mockCommands({ appearance_preferences: () => ({ language: "ja", uiFontFamily: "Old font" }) });
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(delayed.entered()).toBe(true));
  let finishRead!: (value: unknown) => void;
  mockCommands({ appearance_preferences: () => new Promise((resolve) => { finishRead = resolve; }) });
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(finishRead).toBeDefined());
  delayed.finish();
  await vi.waitFor(() => expect(delayed.catalogues.catalogueFor("ja")["wizard.language"]).toBe("言語"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  try {
    expect(useLanguageStore.getState().language).toBe("en");
  } finally {
    finishRead({ language: "de", uiFontFamily: "Newest font" });
  }
  await vi.waitFor(() => expect(useLanguageStore.getState().language).toBe("de"));
});

it("does not publish an obsolete startup fallback after a newer appearance refresh", async () => {
  vi.spyOn(navigator, "languages", "get").mockReturnValue(["ja-JP"]);
  const delayed = await delayCatalogue("ja");
  mockCommands({ appearance_preferences: () => { throw new Error("initial read failed"); } });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const { useLanguageStore } = await import("../../src/state/language-store");
  const installing = installWindowAppearance();
  await vi.waitFor(() => expect(delayed.entered()).toBe(true));
  mockCommands({ appearance_preferences: () => ({ language: "en", uiFontFamily: "Recovered font" }) });
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(useLanguageStore.getState().language).toBe("en"));
  delayed.finish();
  await installing;
  expect(useLanguageStore.getState().language).toBe("en");
});

it("preserves the last good language and reports a current catalogue failure", async () => {
  const catalogues = await import("../../src/i18n/catalogues");
  mockCommands({ appearance_preferences: () => ({ language: "en", uiFontFamily: "Good font" }) });
  const { installWindowAppearance } = await import("../../src/workflows/window-appearance");
  const { useLanguageStore } = await import("../../src/state/language-store");
  await installWindowAppearance();
  vi.spyOn(catalogues, "loadCatalogue").mockRejectedValueOnce(new Error("catalogue unavailable"));
  mockCommands({ appearance_preferences: () => ({ language: "ja", uiFontFamily: "Failed font" }) });
  fireEvent("appearance://changed");
  await vi.waitFor(() => expect(invokeCalls.some((call) => call.command === "record_interface_failure")).toBe(true));
  expect(useLanguageStore.getState().language).toBe("en");
  expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("Good font");
});

it("keeps the wizard's newer staged language when an older catalogue finishes", async () => {
  const delayed = await delayCatalogue("ja");
  const { useWizardStore } = await import("../../src/state/wizard-store");
  useWizardStore.getState().reopen({ sourceDirs: ["/photos"], language: "en" });
  const older = useWizardStore.getState().setLanguage("ja");
  await useWizardStore.getState().setLanguage("de");
  delayed.finish();
  await older;
  expect(useWizardStore.getState().language).toBe("de");
});

it.each(["cancel", "reopen", "init"] as const)("discards a late wizard catalogue after %s", async (action) => {
  const delayed = await delayCatalogue("ja");
  const { useWizardStore } = await import("../../src/state/wizard-store");
  useWizardStore.getState().reopen({ sourceDirs: ["/photos"], language: "en" });
  const older = useWizardStore.getState().setLanguage("ja");
  if (action === "cancel") useWizardStore.getState().cancel();
  else if (action === "reopen") useWizardStore.getState().reopen({ sourceDirs: ["/photos"], language: "de" });
  else await useWizardStore.getState().init({ sourceDirs: [], language: "de" });
  delayed.finish();
  await older;
  expect(useWizardStore.getState().language).toBe(action === "cancel" ? "en" : "de");
});

it("keeps a language choice that finishes loading while Finish saves the earlier draft", async () => {
  const delayed = await delayCatalogue("ja");
  const { useWizardStore } = await import("../../src/state/wizard-store");
  const { finishWizard } = await import("../../src/workflows/wizard");
  let finishSave!: () => void;
  mockCommands({
    save_config: () => new Promise((resolve) => { finishSave = () => resolve({}); }),
    patch_state: () => ({}), check_source_dirs: () => ({ missing: [], substituted: [] }),
    start_source_check: () => true, get_section_counts: () => [], record_recent_notification: () => ({}),
  });
  useWizardStore.getState().reopen({ sourceDirs: ["/photos"], language: "en", defaultTimezone: "UTC" });
  const choosing = useWizardStore.getState().setLanguage("ja");
  // Page navigation remains available during a catalogue import, so Finish
  // can capture the earlier choice before the pending preview publishes.
  useWizardStore.getState().setStep(4);
  const finishing = finishWizard();
  await vi.waitFor(() => expect(finishSave).toBeDefined());
  delayed.finish();
  await choosing;
  finishSave();
  await finishing;
  expect(invokeCalls.find((call) => call.command === "save_config")?.args.changes).toMatchObject({ language: "en" });
  expect(useWizardStore.getState()).toMatchObject({ open: true, finishing: false, language: "ja" });
  expect(useWizardStore.getState().error?.key).toBe("wizard.savedButStale");
});

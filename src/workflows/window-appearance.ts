import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { log, toErrorFields } from "../repositories";
import { createEventInstaller } from "../utils/eventInstallation";
import { createTranslator, message } from "../i18n/translate";
import type { MessageKey } from "../i18n/catalogues";
import type { Language } from "../i18n/languages";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { applyUiFont } from "../utils/uiFont";
import { useLanguageStore } from "../state/language-store";
import { useWindowPreferencesStore } from "../state/window-preferences-store";

// Every window OneCopy draws speaks the interface language in its title
// (interface-language.md L6). Main's title never carries a suffix; the
// others own a translated catalogue key. Identify flashes are excluded: they
// are a fixed brand-only flash.
const WINDOW_TITLE_KEYS: Partial<Record<string, MessageKey>> = {
  preview: "window.titlePreview",
  "fullscreen-view": "window.titleFullscreenView",
  comparison: "window.titleComparison",
};

function currentView(): string | null {
  return new URLSearchParams(window.location.search).get("view");
}

function applyWindowTitle(language: Language): void {
  const view = currentView();
  if (view === null) {
    void getCurrentWindow().setTitle("OneCopy").catch((error) => reportFailure(error));
    return;
  }
  const key = WINDOW_TITLE_KEYS[view];
  if (key === undefined) return;
  const translator = createTranslator(language);
  void getCurrentWindow().setTitle(translator.t(key)).catch((error) => reportFailure(error));
}

interface AppearancePreferences {
  uiFontFamily: unknown;
  language?: unknown;
  systemLanguage?: unknown;
  systemLocale?: unknown;
  enlargeSmallImages?: unknown;
  videoTranscriptionEnabled?: unknown;
  audioTranscriptionEnabled?: unknown;
}

async function readPreferences(): Promise<AppearancePreferences> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      invoke<AppearancePreferences>("appearance_preferences"),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("Appearance read timed out")), 5_000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function reportFailure(error: unknown): void {
  log.warn("window appearance update failed", toErrorFields(error));
  recordInterfaceFailure(message("app.appearanceUpdateFailed"));
}

// All routes share this small auxiliary-window read model, never Main's
// library/bootstrap data. The theme is not part of it: the Rust core sets each
// window's theme natively and App.css follows through prefers-color-scheme.
// A saved-config event invalidates pending reads so a late old response cannot
// replace a newer font. Failed refreshes preserve the last good view. The
// invalidation is broadcast, so a hidden or reused window takes a new
// language too.
export const installWindowAppearance = createEventInstaller(async (listeners) => {
  applyUiFont(undefined);
  let request = 0;
  const refresh = async () => {
    const current = ++request;
    try {
      const preferences = await readPreferences();
      if (current !== request) return;
      await useLanguageStore.getState().apply(preferences);
      if (current !== request) return;
      applyUiFont(preferences.uiFontFamily);
      applyWindowTitle(useLanguageStore.getState().language);
      useWindowPreferencesStore.getState().apply(preferences);
    } catch (error) {
      if (current === request) reportFailure(error);
    }
  };
  await listeners.listen("appearance://changed", () => { void refresh(); });
  await refresh();
}, reportFailure);

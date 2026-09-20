import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { createEventInstaller } from "../utils/eventInstallation";
import { message } from "../i18n/translate";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { applyUiFont } from "../utils/uiFont";
import { useLanguageStore } from "../state/language-store";
import { useWindowPreferencesStore } from "../state/window-preferences-store";

interface AppearancePreferences {
  uiFontFamily: unknown;
  language?: unknown;
  systemLanguage?: unknown;
  systemLocale?: unknown;
  enlargeSmallImagesInPreview?: unknown;
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
// replace a newer font. Failed refreshes preserve the last good view.
export const installWindowAppearance = createEventInstaller(async (listeners) => {
  applyUiFont(undefined);
  let request = 0;
  const refresh = async () => {
    const current = ++request;
    try {
      const preferences = await readPreferences();
      if (current !== request) return;
      applyUiFont(preferences.uiFontFamily);
      useLanguageStore.getState().apply(preferences);
      useWindowPreferencesStore.getState().apply(preferences);
    } catch (error) {
      if (current === request) reportFailure(error);
    }
  };
  await listeners.listen("appearance://changed", () => { void refresh(); });
  await refresh();
}, reportFailure);

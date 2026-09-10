import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import { createEventInstaller } from "../utils/eventInstallation";
import { recordInterfaceFailure } from "../utils/failureSurface";
import { applyTheme, applyUiFont, watchSystemTheme } from "../utils/theme";
import { useWindowPreferencesStore } from "../state/window-preferences-store";

interface AppearancePreferences {
  theme: unknown;
  uiFontFamily: unknown;
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
  recordInterfaceFailure("Couldn’t update this window’s appearance. Saved settings were not changed.");
}

// All routes share this small auxiliary-window read model, never Main's
// library/bootstrap data.
// A saved-config event invalidates pending reads so a late old response cannot
// replace a newer theme or font. Failed refreshes preserve the last good view.
export const installWindowAppearance = createEventInstaller(async (listeners) => {
  applyTheme("system");
  applyUiFont(undefined);
  listeners.retain(watchSystemTheme());
  let request = 0;
  const refresh = async () => {
    const current = ++request;
    try {
      const preferences = await readPreferences();
      if (current !== request) return;
      applyTheme(preferences.theme);
      applyUiFont(preferences.uiFontFamily);
      useWindowPreferencesStore.getState().apply(preferences);
    } catch (error) {
      if (current === request) reportFailure(error);
    }
  };
  await listeners.listen("appearance://changed", () => { void refresh(); });
  await refresh();
}, reportFailure);

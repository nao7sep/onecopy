// The app-level startup data (config + state + data root), owned in one store
// so every surface that needs the current config reads one source of truth —
// and a settings save can refresh it everywhere at once.
//
// This store is the ONE writer for both persisted documents: every mutation
// goes through patchConfig/patchState, which send only the changed keys, let
// the core merge into the file it holds, and publish the merged result here.
// No caller ever spreads a cached copy over the file again.

import { create } from "zustand";
import {
  loadAppData,
  log,
  patchConfigFile,
  patchStateFile,
  toErrorFields,
  type LoadedAppData,
} from "../repositories";
import { applyTheme } from "../utils/theme";

interface AppState {
  appData: LoadedAppData | null;
  loadError: string | null;
  reload: () => Promise<void>;
  patchConfig: (patch: Record<string, unknown>) => Promise<void>;
  patchState: (patch: Record<string, unknown>) => Promise<void>;
}

// State writes are debounced and coalesced: selection/zoom/pane state can
// change per keystroke, and one write per pause is plenty (the backup store
// dedups identical content, but churn is churn).
let pendingStatePatch: Record<string, unknown> | null = null;
let stateFlushTimer: ReturnType<typeof setTimeout> | null = null;
const STATE_FLUSH_MS = 400;

export const useAppStore = create<AppState>((set) => ({
  appData: null,
  loadError: null,

  patchConfig: async (patch) => {
    try {
      const merged = await patchConfigFile(patch);
      set((s) =>
        s.appData === null ? s : { appData: { ...s.appData, config: merged } },
      );
      // The main window re-themes live; other windows apply at their load.
      applyTheme(merged.theme);
    } catch (error) {
      log.error("config patch failed", toErrorFields(error));
      throw error;
    }
  },

  patchState: async (patch) => {
    // Publish optimistically so readers see the new state immediately; the
    // disk write coalesces on a short timer.
    set((s) =>
      s.appData === null
        ? s
        : { appData: { ...s.appData, state: { ...(s.appData.state ?? {}), ...patch } } },
    );
    pendingStatePatch = { ...(pendingStatePatch ?? {}), ...patch };
    if (stateFlushTimer !== null) clearTimeout(stateFlushTimer);
    stateFlushTimer = setTimeout(() => {
      const toWrite = pendingStatePatch;
      pendingStatePatch = null;
      stateFlushTimer = null;
      if (toWrite === null) return;
      patchStateFile(toWrite).catch((error) => {
        log.error("state patch failed", toErrorFields(error));
      });
    }, STATE_FLUSH_MS);
  },

  reload: async () => {
    try {
      const data = await loadAppData();
      set({ appData: data, loadError: null });
      log.info("app data loaded", {
        dataRoot: data.dataRoot,
        hasConfig: data.config !== null,
        hasState: data.state !== null,
      });
      const { useWizardStore } = await import("./wizard-store");
      await useWizardStore.getState().init(data.config, data.dataRoot);
      const { useDestinationsStore } = await import("./destinations-store");
      useDestinationsStore.getState().init(data.config);
    } catch (error) {
      set({ loadError: String(error) });
      log.error("app data load failed", toErrorFields(error));
    }
  },
}));

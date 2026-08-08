// The app-level startup data (config + state + data root), owned in one store
// so every surface that needs the current config reads one source of truth —
// and a settings save can refresh it everywhere at once.

import { create } from "zustand";
import { loadAppData, log, toErrorFields, type LoadedAppData } from "../repositories";

interface AppState {
  appData: LoadedAppData | null;
  loadError: string | null;
  reload: () => Promise<void>;
}

export const useAppStore = create<AppState>((set) => ({
  appData: null,
  loadError: null,

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

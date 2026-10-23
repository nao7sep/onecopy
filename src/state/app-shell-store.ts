import { create } from "zustand";
import { useAppStore } from "./app-store";
import { useSettingsStore } from "./settings-store";

export type UtilitySurface =
  | "about"
  | "backgroundWork"
  | "deletedFiles"
  | "issues"
  | "settings"
  | "shortcuts";

interface AppShellState {
  utilitySurface: UtilitySurface | null;
  openUtility: (surface: UtilitySurface) => void;
  closeUtility: () => void;
}

/**
 * The main window owns which top-level utility surface is visible. Feature
 * stores own the data and operations rendered inside those surfaces, never
 * presentation routing. This union makes two competing utility modals
 * unrepresentable without coupling the feature stores to one another.
 */
export const useAppShellStore = create<AppShellState>((set) => ({
  utilitySurface: null,
  openUtility: (utilitySurface) => {
    // Every Settings route, including recovery links replacing another
    // utility, needs its draft before the modal becomes visible. Reopening
    // must not overwrite edits already held by the Settings owner.
    if (utilitySurface === "settings" && useSettingsStore.getState().draft === null) {
      const appData = useAppStore.getState().appData;
      if (appData === null) return;
      useSettingsStore.getState().beginEditing(
        appData.config,
        appData.aiAccelerationCapabilities,
        appData.configDefaults,
      );
    }
    set({ utilitySurface });
  },
  closeUtility: () => set({ utilitySurface: null }),
}));

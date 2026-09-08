import { create } from "zustand";

export type UtilitySurface =
  | "about"
  | "activityTrace"
  | "backgroundWork"
  | "deletedFiles"
  | "issues"
  | "managedTools"
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
  openUtility: (utilitySurface) => set({ utilitySurface }),
  closeUtility: () => set({ utilitySurface: null }),
}));

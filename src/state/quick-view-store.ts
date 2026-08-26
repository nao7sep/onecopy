import { create } from "zustand";

interface QuickViewState {
  open: boolean;
  show: () => void;
  close: () => void;
}

export const useQuickViewStore = create<QuickViewState>((set) => ({
  open: false,
  show: () => set({ open: true }),
  close: () => set({ open: false }),
}));

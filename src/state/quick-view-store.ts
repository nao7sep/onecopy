import { create } from "zustand";
import { itemKey, useItemsStore } from "./items-store";

interface QuickViewState {
  open: boolean;
  show: () => boolean;
  close: () => void;
}

export const useQuickViewStore = create<QuickViewState>((set) => ({
  open: false,
  show: () => {
    const { selected, selectedItem, items } = useItemsStore.getState();
    if (selected?.kind !== "image" && selected?.kind !== "video") return false;
    if (selectedItem === null || !items.some((item) => itemKey(item) === selectedItem)) {
      return false;
    }
    set({ open: true });
    return true;
  },
  close: () => set({ open: false }),
}));

function focusedVideoOwnsSpace(): boolean {
  if (typeof document === "undefined" || typeof Element === "undefined") return false;
  const active = document.activeElement;
  return active instanceof Element && active.closest("[data-video-surface]") !== null;
}

/** Main and grid key layers share this exact routing decision. */
export function handleSpaceQuickView(event: {
  preventDefault: () => void;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}): boolean {
  if (event.metaKey || event.ctrlKey || event.altKey) return false;
  if (focusedVideoOwnsSpace()) return false;
  if (!useQuickViewStore.getState().show()) return false;
  event.preventDefault();
  return true;
}

// The selected section and its grid items.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import type { SectionItem } from "../models/items";

export interface SelectedSection {
  kind: "image" | "video" | "other";
  month: string;
}

/** The grid's stable identity for a logical item. */
export function itemKey(item: SectionItem): string {
  return item.hash ?? `path-${item.pathId}`;
}

interface ItemsState {
  selected: SelectedSection | null;
  items: SectionItem[];
  loading: boolean;
  selectedItem: string | null;
  select: (section: SelectedSection) => Promise<void>;
  selectItem: (key: string | null) => void;
  deleteSelected: (permanent: boolean) => Promise<void>;
  refresh: () => Promise<void>;
}

export const useItemsStore = create<ItemsState>((set, get) => ({
  selected: null,
  items: [],
  loading: false,
  selectedItem: null,

  select: async (section) => {
    set({ selected: section, loading: true, selectedItem: null });
    try {
      const items = await invoke<SectionItem[]>("get_section_items", {
        kind: section.kind,
        month: section.month,
      });
      // Ignore a stale response if the selection moved on meanwhile.
      if (get().selected === section) {
        set({ items, loading: false });
      }
    } catch (error) {
      log.error("section items load failed", toErrorFields(error));
      set({ items: [], loading: false });
    }
  },

  selectItem: (key) => set({ selectedItem: key }),

  // Deletes the selected logical item — every copy plus companions — to trash
  // (or permanently with Shift). Selection recovers onto the next item so a
  // cull run keeps its keyboard rhythm.
  deleteSelected: async (permanent) => {
    const { items, selectedItem, refresh } = get();
    const index = items.findIndex((i) => itemKey(i) === selectedItem);
    if (index < 0) return;
    const item = items[index];
    try {
      await invoke("delete_item", {
        hash: item.hash,
        pathId: item.hash === null ? item.pathId : null,
        permanent,
      });
      const next = items[index + 1] ?? items[index - 1] ?? null;
      set({ selectedItem: next ? itemKey(next) : null });
      await refresh();
      const { useSectionsStore } = await import("./sections-store");
      await useSectionsStore.getState().loadCounts();
    } catch (error) {
      log.error("delete failed", toErrorFields(error));
    }
  },

  refresh: async () => {
    const { selected, select } = get();
    if (selected) {
      const kept = get().selectedItem;
      await select(selected);
      set({ selectedItem: kept });
    }
  },
}));

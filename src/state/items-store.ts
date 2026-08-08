// The selected section and its grid items.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import type { SectionItem } from "../models/items";

export interface SelectedSection {
  kind: "image" | "video" | "other";
  month: string;
}

interface ItemsState {
  selected: SelectedSection | null;
  items: SectionItem[];
  loading: boolean;
  select: (section: SelectedSection) => Promise<void>;
  refresh: () => Promise<void>;
}

export const useItemsStore = create<ItemsState>((set, get) => ({
  selected: null,
  items: [],
  loading: false,

  select: async (section) => {
    set({ selected: section, loading: true });
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

  refresh: async () => {
    const { selected, select } = get();
    if (selected) await select(selected);
  },
}));

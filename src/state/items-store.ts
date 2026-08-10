// The selected section and its grid items.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { log, toErrorFields } from "../repositories";
import type { SectionItem, SortOrder } from "../models/items";

export interface SelectedSection {
  kind: "image" | "video" | "other";
  month: string;
}

// Mirrors queries::ItemDetail.
export interface ItemDetail {
  fileName: string;
  kind: string;
  byteSize: number | null;
  width: number | null;
  height: number | null;
  durationMs: number | null;
  resolvedUtcMs: number | null;
  resolvedSource: string | null;
  dateOnly: boolean;
  copyPaths: string[];
  companionPaths: string[];
  stripFrames: number | null;
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
  /** The full multi-selection (always contains the anchor when non-empty). */
  selectedKeys: Set<string>;
  detail: ItemDetail | null;
  sortOrder: SortOrder;
  setSortOrder: (order: SortOrder) => void;
  select: (section: SelectedSection) => Promise<void>;
  selectItem: (key: string | null) => void;
  toggleItem: (key: string) => void;
  rangeSelect: (sortedKeys: string[], key: string) => void;
  deleteSelected: (permanent: boolean) => Promise<void>;
  rescanSection: () => Promise<void>;
  refresh: () => Promise<void>;
}

export const useItemsStore = create<ItemsState>((set, get) => ({
  selected: null,
  items: [],
  loading: false,
  selectedItem: null,
  selectedKeys: new Set<string>(),
  detail: null,
  sortOrder: "time",

  setSortOrder: (order) => {
    set({ sortOrder: order });
    void import("./app-store").then(({ useAppStore }) =>
      useAppStore.getState().patchState({ sortOrder: order }),
    );
  },

  select: async (section) => {
    // A same-section reload (refresh after delete/rescan) keeps the stale
    // items rendered so the grid's scroll container never unmounts — the
    // scroll position survives. A real section switch clears them.
    const previous = get().selected;
    const sameSection =
      previous !== null &&
      previous.kind === section.kind &&
      previous.month === section.month;
    set({
      selected: section,
      loading: true,
      selectedItem: null,
      selectedKeys: new Set(),
      detail: null,
      ...(sameSection ? {} : { items: [] }),
    });
    void import("./app-store").then(({ useAppStore }) =>
      useAppStore.getState().patchState({ lastSection: section }),
    );
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

  selectItem: (key) => {
    set({
      selectedItem: key,
      selectedKeys: key === null ? new Set() : new Set([key]),
      detail: null,
    });
    // The anchor persists (debounced by the state owner, so arrow scrubbing
    // costs one write per pause), letting a relaunch land where culling left.
    void import("./app-store").then(({ useAppStore }) =>
      useAppStore.getState().patchState({ lastItem: key }),
    );
    if (key === null) return;
    const item = get().items.find((i) => itemKey(i) === key);
    if (!item) return;
    // An open preview window follows the selection live.
    void import("./preview-store").then(({ updatePreviewIfOpen }) =>
      updatePreviewIfOpen({
        hash: item.hash,
        pathId: item.hash === null ? item.pathId : null,
      }),
    );
    void invoke<ItemDetail>("get_item_detail", {
      hash: item.hash,
      pathId: item.hash === null ? item.pathId : null,
    })
      .then((detail) => {
        // Ignore a stale response if the selection moved on meanwhile.
        if (get().selectedItem === key) set({ detail });
      })
      .catch((error) => {
        log.error("item detail load failed", toErrorFields(error));
      });
  },

  toggleItem: (key) => {
    const next = new Set(get().selectedKeys);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    // The anchor moves to the toggled key (or clears with the selection).
    set({ selectedKeys: next, selectedItem: next.has(key) ? key : null });
  },

  rangeSelect: (sortedKeys, key) => {
    const { selectedItem, selectedKeys } = get();
    const from = selectedItem !== null ? sortedKeys.indexOf(selectedItem) : -1;
    const to = sortedKeys.indexOf(key);
    if (from < 0 || to < 0) return;
    const [start, end] = from <= to ? [from, to] : [to, from];
    const next = new Set(selectedKeys);
    for (const k of sortedKeys.slice(start, end + 1)) next.add(k);
    set({ selectedKeys: next });
  },

  // Deletes every selected logical item — copies plus companions each — to
  // trash (or permanently with Shift). Selection recovers onto the next
  // unselected item so a cull run keeps its keyboard rhythm.
  deleteSelected: async (permanent) => {
    const { items, selectedItem, selectedKeys, refresh } = get();
    const keys = selectedKeys.size > 0
      ? selectedKeys
      : selectedItem !== null
        ? new Set([selectedItem])
        : new Set<string>();
    if (keys.size === 0) return;
    const anchorIndex = items.findIndex((i) => itemKey(i) === selectedItem);
    try {
      for (const item of items.filter((i) => keys.has(itemKey(i)))) {
        await invoke("delete_item", {
          hash: item.hash,
          pathId: item.hash === null ? item.pathId : null,
          permanent,
        });
      }
      const survivor =
        items.slice(anchorIndex + 1).find((i) => !keys.has(itemKey(i))) ??
        [...items.slice(0, Math.max(anchorIndex, 0))]
          .reverse()
          .find((i) => !keys.has(itemKey(i))) ??
        null;
      const survivorKey = survivor ? itemKey(survivor) : null;
      set({
        selectedItem: survivorKey,
        selectedKeys: survivorKey ? new Set([survivorKey]) : new Set(),
      });
      await refresh();
      const { useSectionsStore } = await import("./sections-store");
      await useSectionsStore.getState().loadCounts();
    } catch (error) {
      log.error("delete failed", toErrorFields(error));
    }
  },

  // Scoped rescan: re-stats only the directories that contributed to the open
  // section (the full per-root walk stays behind the Scan button).
  rescanSection: async () => {
    const { selected, refresh } = get();
    if (!selected) return;
    try {
      await invoke<number>("rescan_section", {
        kind: selected.kind,
        month: selected.month,
      });
      await refresh();
      const { useSectionsStore } = await import("./sections-store");
      await useSectionsStore.getState().loadCounts();
    } catch (error) {
      log.error("section rescan failed", toErrorFields(error));
    }
  },

  refresh: async () => {
    const { selected, select } = get();
    if (selected) {
      const keptAnchor = get().selectedItem;
      const keptKeys = get().selectedKeys;
      await select(selected);
      // Restore what survived the refresh.
      const alive = new Set(get().items.map(itemKey));
      const keys = new Set([...keptKeys].filter((k) => alive.has(k)));
      const anchor = keptAnchor !== null && alive.has(keptAnchor) ? keptAnchor : null;
      set({ selectedItem: anchor, selectedKeys: keys });
    }
  },
}));

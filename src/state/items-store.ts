// The selected section and its grid items.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { requestSeq } from "./request-seq";
import { log, toErrorFields } from "../repositories";
import { replaceDerivedItem, sortItems } from "../models/items";
import { DEFAULT_DESC, SORT_ORDERS, type SectionItem, type SortChoice, type SortOrder } from "../models/items";

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

/** Mirrors operations::DeleteOutcome. */
interface DeleteOutcome {
  deletedFiles: number;
  failedFiles: number;
  removedRows: number;
}

interface ItemsState {
  selected: SelectedSection | null;
  items: SectionItem[];
  loading: boolean;
  selectedItem: string | null;
  /** The full multi-selection (always contains the anchor when non-empty). */
  selectedKeys: Set<string>;
  /** Where the current Shift range grows FROM. Held apart from the anchor
   * because Shift+arrow moves the anchor on every press, so an anchor-based
   * origin can only ever extend — the range could never be narrowed. */
  rangeOrigin: string | null;
  /** The selection as it stood when the current range began. Each Shift
   * gesture rebuilds from this, so the span can shrink without discarding
   * keys that were Cmd-clicked outside it. */
  rangeBase: Set<string>;
  detail: ItemDetail | null;
  /** Sort per LANE — other-files sort like a file manager (name, kind),
   * photos and videos sort like a light table (time taken, resolution); one
   * shared order let each kind be shown in orders that are nonsense for it
   * ("Time taken" over files nobody took). Each lane carries its direction;
   * `currentSort()` resolves the active lane from the open section. */
  sortOrders: { media: SortChoice; other: SortChoice };
  currentSort: () => SortChoice;
  /** Last outcome worth showing the user (a delete that failed on disk, or a
   * command that was refused). Null whenever the last action was clean. */
  message: string | null;
  setSortOrder: (order: SortOrder) => void;
  select: (section: SelectedSection) => Promise<void>;
  selectItem: (key: string | null) => void;
  /** Moves the anchor WITHOUT collapsing the multi-selection (Shift+arrow). */
  setAnchor: (key: string | null) => void;
  toggleItem: (key: string) => void;
  rangeSelect: (sortedKeys: string[], key: string) => void;
  deleteSelected: (permanent: boolean) => Promise<void>;
  /** Deletes an explicit set, for surfaces scoped to one item. */
  deleteKeys: (keys: Set<string>, permanent: boolean) => Promise<void>;
  rescanSection: () => Promise<void>;
  refresh: () => Promise<void>;
  applyDerivedItem: (previousHash: string, item: SectionItem) => void;
  /** After a similar-family is fully decided, land the anchor on the first
   * item PAST the family (in the shown order), so Enter chains straight into
   * the next group. Past the KEEPERS, deliberately: Enter on a keeper would
   * reopen the family just decided. */
  selectAfterFamily: (memberHashes: string[]) => void;
}

// One guard per query the store issues (request-seq.ts explains why: these
// reads are async commands now, so responses can arrive out of order).
const sectionLoad = requestSeq();
const detailLoad = requestSeq();

export const useItemsStore = create<ItemsState>((set, get) => ({
  selected: null,
  items: [],
  loading: false,
  selectedItem: null,
  selectedKeys: new Set<string>(),
  rangeOrigin: null,
  rangeBase: new Set<string>(),
  detail: null,
  sortOrders: {
    media: SORT_ORDERS.media.defaultChoice,
    other: SORT_ORDERS.other.defaultChoice,
  },
  message: null,

  currentSort: () => {
    const { selected, sortOrders } = get();
    return selected?.kind === "other" ? sortOrders.other : sortOrders.media;
  },

  setSortOrder: (order) => {
    const lane = get().selected?.kind === "other" ? "other" : "media";
    const current = get().sortOrders[lane];
    // Picking the ACTIVE order again flips its direction (the header-click
    // convention); a fresh order starts in its natural direction.
    const next: SortChoice =
      current.order === order
        ? { order, desc: !current.desc }
        : { order, desc: DEFAULT_DESC[order] };
    const sortOrders = { ...get().sortOrders, [lane]: next };
    set({ sortOrders });
    void import("./app-store").then(({ useAppStore }) =>
      useAppStore.getState().patchState({ sortOrders }),
    );
  },

  select: async (section) => {
    const previous = get().selected;
    const sameSection =
      previous !== null &&
      previous.kind === section.kind &&
      previous.month === section.month;
    // A same-section reload (refresh after a delete, a rescan, or a watcher
    // event) keeps the stale items rendered so the grid's scroll container
    // never unmounts, AND keeps the selection live for the whole round trip.
    // Blanking it here would make every arrow key landing mid-reload select
    // item zero and every Delete a silent no-op, for as long as the query
    // takes. A real section switch clears both.
    set({
      selected: section,
      loading: true,
      ...(sameSection
        ? {}
        : {
            items: [],
            selectedItem: null,
            selectedKeys: new Set<string>(),
            rangeOrigin: null,
            rangeBase: new Set<string>(),
            detail: null,
          }),
    });
    void import("./app-store").then(({ useAppStore }) =>
      useAppStore.getState().patchState({ lastSection: section }),
    );
    // Sequence, not identity: `refresh` passes the SAME section object, so an
    // identity check cannot tell two same-section reloads apart — with the
    // reads async, an older response landing last would resurrect rows a
    // newer reload had already dropped.
    const fresh = sectionLoad.begin();
    try {
      const items = await invoke<SectionItem[]>("get_section_items", {
        kind: section.kind,
        month: section.month,
      });
      if (fresh()) {
        set({ items, loading: false });
        if (sameSection) dropVanishedSelection(set, get);
      }
    } catch (error) {
      log.error("section items load failed", toErrorFields(error));
      // Only the latest request may blank — a rejection for a section the
      // user already navigated away from must not wipe the live one.
      if (fresh()) {
        set({ items: [], loading: false });
      }
    }
  },

  selectItem: (key) => {
    set({
      selectedItem: key,
      selectedKeys: key === null ? new Set() : new Set([key]),
      rangeOrigin: key,
      rangeBase: key === null ? new Set<string>() : new Set([key]),
      detail: null,
    });
    notifyAnchor(key);
  },

  // Moves the anchor only. The range origin deliberately stays put, so a
  // Shift+arrow run can reverse and shrink instead of only growing.
  setAnchor: (key) => {
    set({ selectedItem: key });
    notifyAnchor(key);
  },

  toggleItem: (key) => {
    const next = new Set(get().selectedKeys);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    // The anchor moves to the toggled key; toggling the anchor OFF falls back
    // to the most recently selected item remaining — a Set preserves
    // insertion order, so that is its last element. Cmd-clicking through a
    // pile and then un-clicking the last one previews the one before it,
    // which is what "which one is selected?" needs during a multi-select.
    const anchor = next.has(key) ? key : ([...next].pop() ?? null);
    set({
      selectedKeys: next,
      selectedItem: anchor,
      rangeOrigin: key,
      rangeBase: new Set(next),
    });
    notifyAnchor(anchor);
  },

  // Recomputes the range as the span from the origin to the target on top of
  // the selection as it stood when the range began, rather than unioning into
  // the live selection: shift-clicking (or shift-arrowing) back toward the
  // origin means "I overshot, take it back", and a union can never express
  // that. Rebuilding from `rangeBase` is what lets the span shrink while
  // Cmd-clicked keys outside it survive.
  rangeSelect: (sortedKeys, key) => {
    const { rangeOrigin, selectedItem, rangeBase } = get();
    const origin = rangeOrigin ?? selectedItem;
    const from = origin !== null ? sortedKeys.indexOf(origin) : -1;
    const to = sortedKeys.indexOf(key);
    if (from < 0 || to < 0) return;
    const [start, end] = from <= to ? [from, to] : [to, from];
    const next = new Set(rangeBase);
    for (const k of sortedKeys.slice(start, end + 1)) next.add(k);
    set({ selectedKeys: next });
  },

  // Deletes every selected logical item — copies plus companions each — to
  // trash (or permanently with Shift). Selection recovers onto the next
  // unselected item so a cull run keeps its keyboard rhythm.
  deleteSelected: async (permanent) => {
    const { selectedItem, selectedKeys, deleteKeys } = get();
    const keys = selectedKeys.size > 0
      ? selectedKeys
      : selectedItem !== null
        ? new Set([selectedItem])
        : new Set<string>();
    await deleteKeys(keys, permanent);
  },

  // Deletes an explicit set. Surfaces that act on ONE item — the scenes modal
  // opens on the anchor and its footer promises it acts on that video — call
  // this rather than deleteSelected, whose target is whatever is selected in
  // the grid behind them.
  deleteKeys: async (keys, permanent) => {
    const { items, selectedItem, refresh } = get();
    if (keys.size === 0) return;
    // Recovery walks the order the GRID renders, not the backend's — under
    // any sort but "time" the two diverge, and recovering through the backend
    // order lands the ring on a tile the user is not looking at.
    const shown = sortItems(items, get().currentSort());
    // With the anchor toggled off, the deleted run still has a position: use
    // the first selected tile, so recovery stays adjacent instead of falling
    // back to index 0 and scrolling the grid to the top.
    const anchorIndex =
      selectedItem !== null
        ? shown.findIndex((i) => itemKey(i) === selectedItem)
        : shown.findIndex((i) => keys.has(itemKey(i)));
    set({ message: null });
    try {
      let failed = 0;
      for (const item of shown.filter((i) => keys.has(itemKey(i)))) {
        const outcome = await invoke<DeleteOutcome>("delete_item", {
          hash: item.hash,
          pathId: item.hash === null ? item.pathId : null,
          permanent,
        });
        failed += outcome?.failedFiles ?? 0;
      }
      const survivor =
        shown.slice(anchorIndex + 1).find((i) => !keys.has(itemKey(i))) ??
        [...shown.slice(0, Math.max(anchorIndex, 0))]
          .reverse()
          .find((i) => !keys.has(itemKey(i))) ??
        null;
      const survivorKey = survivor ? itemKey(survivor) : null;
      // A per-copy failure is reported in the outcome, never as a rejection,
      // and leaves the file on disk. Saying so is the whole difference
      // between "Delete does nothing" and "that drive is read-only".
      if (failed > 0) {
        set({
          message: `${failed} file${failed === 1 ? "" : "s"} could not be deleted — see Issues.`,
        });
        const { useIssuesStore } = await import("./issues-store");
        await useIssuesStore.getState().load();
      }
      // ORDER (Phase 33, the Windows walk): rows vanish FIRST, then focus
      // lands where the deleted item was — follower, else previous, else
      // none, the standard file-manager rhythm. Setting the survivor before
      // the refresh made the focus ring visibly hop to a neighbour while the
      // doomed tiles were still on screen.
      await refresh();
      set({
        selectedItem: survivorKey,
        selectedKeys: survivorKey ? new Set([survivorKey]) : new Set(),
        rangeOrigin: survivorKey,
        rangeBase: survivorKey ? new Set([survivorKey]) : new Set<string>(),
      });
      // The recovery selection is an anchor move: the preview must stop
      // showing the file that was just trashed.
      notifyAnchor(survivorKey);
      const { useSectionsStore } = await import("./sections-store");
      await useSectionsStore.getState().loadCounts();
    } catch (error) {
      log.error("delete failed", toErrorFields(error));
      set({ message: messageOf(error) });
    }
  },

  selectAfterFamily: (memberHashes) => {
    const { items } = get();
    const family = new Set(memberHashes);
    const shown = sortItems(items, get().currentSort());
    const lastMember = shown.reduce(
      (last, item, index) => (item.hash !== null && family.has(item.hash) ? index : last),
      -1,
    );
    const next =
      shown.slice(lastMember + 1).find((item) => item.hash === null || !family.has(item.hash)) ??
      // The family sat at the end: rest on its last keeper rather than
      // leaving the anchor on a trashed item.
      (lastMember >= 0 ? shown[lastMember] : undefined);
    if (next) {
      get().selectItem(itemKey(next));
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

  // A same-section reload. `select` now carries the selection across it and
  // reconciles once the rows land, so there is nothing to save and restore.
  refresh: async () => {
    const { selected, select } = get();
    if (selected) await select(selected);
  },

  applyDerivedItem: (previousHash, item) => {
    const state = get();
    const items = replaceDerivedItem(state.items, previousHash, item);
    if (items === state.items || item.hash === null) return;
    const current = item.hash;
    const remap = (key: string | null): string | null =>
      key === previousHash ? current : key;
    const remapKey = (key: string): string => (key === previousHash ? current : key);
    const remapSet = (keys: Set<string>): Set<string> =>
      new Set([...keys].map(remapKey));
    const selectedItem = remap(state.selectedItem);
    set({
      items,
      selectedItem,
      selectedKeys: remapSet(state.selectedKeys),
      rangeOrigin: remap(state.rangeOrigin),
      rangeBase: remapSet(state.rangeBase),
    });
    if (selectedItem === current) notifyAnchor(current);
  },
}));

/** Drops whatever the reload did not bring back, leaving everything that
 * survived exactly where it was. Only reached on a same-section reload; a
 * real section switch clears the selection outright. */
function dropVanishedSelection(
  set: (partial: Partial<ItemsState>) => void,
  get: () => ItemsState,
): void {
  const { items, selectedItem, selectedKeys, rangeOrigin, rangeBase } = get();
  const alive = new Set(items.map(itemKey));
  const keys = new Set([...selectedKeys].filter((k) => alive.has(k)));
  const anchor =
    selectedItem !== null && alive.has(selectedItem) ? selectedItem : null;
  set({
    selectedItem: anchor,
    selectedKeys: keys,
    rangeOrigin: rangeOrigin !== null && alive.has(rangeOrigin) ? rangeOrigin : anchor,
    rangeBase: new Set([...rangeBase].filter((k) => alive.has(k))),
  });
  if (anchor !== selectedItem) notifyAnchor(anchor);
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Every path that moves the anchor funnels here: the persisted `lastItem`,
 * the preview follow (throttled in the preview store), and the ONE detail
 * fetch — whose result both fills the metadata pane and completes the
 * preview's message, so nothing queries twice and nothing races. */
function notifyAnchor(key: string | null): void {
  void import("./app-store").then(({ useAppStore }) =>
    useAppStore.getState().patchState({ lastItem: key }),
  );
  if (key === null) {
    // The selection emptied: the preview must BLANK, not hold the previous
    // photo — for a trashed file the hold was a small lie. This return used
    // to come before the preview heard anything.
    void import("./preview-store").then(({ usePreviewStore }) =>
      usePreviewStore.getState().anchorCleared(),
    );
    return;
  }
  const item = useItemsStore.getState().items.find((i) => itemKey(i) === key);
  if (!item) return;
  const payload = { hash: item.hash, pathId: item.hash === null ? item.pathId : null };
  void import("./preview-store").then(({ usePreviewStore }) =>
    usePreviewStore.getState().anchorChanged(payload, null),
  );
  // Both guards: the key check drops a response for an anchor the user left,
  // the sequence drops the OLDER of two responses for the same anchor.
  const fresh = detailLoad.begin();
  void invoke<ItemDetail>("get_item_detail", payload)
    .then((detail) => {
      if (fresh() && useItemsStore.getState().selectedItem === key) {
        useItemsStore.setState({ detail });
        void import("./preview-store").then(({ usePreviewStore }) =>
          usePreviewStore.getState().detailLoaded(payload, detail),
        );
      }
    })
    .catch((error) => {
      log.error("item detail load failed", toErrorFields(error));
    });
}

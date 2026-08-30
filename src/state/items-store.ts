// The selected section and its grid items.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { requestSeq } from "./request-seq";
import { log, toErrorFields } from "../repositories";
import { replaceDerivedItem, sortItems } from "../models/items";
import { DEFAULT_DESC, SORT_ORDERS, type ItemDetail, type SectionItem, type SortChoice, type SortOrder } from "../models/items";
import {
  anchorContext,
  recoverAnchor,
  type AnchorContext,
  type SectionMemory,
} from "../models/mainSelection";

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
  loadError: string | null;
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
  /** Per-section work positions live only for this app run. The persisted
   * state owns only the last-open section's bounded context. */
  sectionMemory: Record<string, SectionMemory>;
  scrollRequest: { key: string; align: "nearest" | "center"; id: number } | null;
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
  select: (
    section: SelectedSection,
    restore?: { anchor: string | null; context: AnchorContext | null },
  ) => Promise<void>;
  selectItem: (key: string | null, align?: "nearest" | "center") => void;
  /** Moves the anchor WITHOUT collapsing the multi-selection (Shift+arrow). */
  setAnchor: (key: string | null) => void;
  toggleItem: (key: string) => void;
  rangeSelect: (sortedKeys: string[], key: string) => void;
  refresh: () => Promise<void>;
  applyDerivedItem: (previousHash: string, item: SectionItem) => void;
  /** After a similar-family is fully decided, land the anchor on the first
   * item PAST the family (in the shown order), so Enter chains straight into
   * the next group. Past the retained images, deliberately: Enter on one would
   * reopen the family just decided. */
  selectAfterFamily: (
    memberHashes: string[],
    orderBeforeComparison: SectionItem[],
  ) => void;
}

// One guard per query the store issues (request-seq.ts explains why: these
// reads are async commands now, so responses can arrive out of order).
const sectionLoad = requestSeq();
const detailLoad = requestSeq();
let scrollRequestId = 0;

function requestScroll(key: string, align: "nearest" | "center") {
  scrollRequestId += 1;
  return { key, align, id: scrollRequestId };
}

export const useItemsStore = create<ItemsState>((set, get) => ({
  selected: null,
  items: [],
  loading: false,
  loadError: null,
  selectedItem: null,
  selectedKeys: new Set<string>(),
  rangeOrigin: null,
  rangeBase: new Set<string>(),
  sectionMemory: {},
  scrollRequest: null,
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
    const state = get();
    const anchor = state.selectedItem;
    set({
      sortOrders,
      scrollRequest:
        anchor === null ? state.scrollRequest : requestScroll(anchor, "center"),
    });
  },

  select: async (section, restore) => {
    const before = get();
    const previous = before.selected;
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
    const memory = { ...before.sectionMemory };
    if (!sameSection && previous !== null) {
      const order = sortItems(before.items, before.currentSort()).map(itemKey);
      memory[sectionId(previous)] = {
        anchor: before.selectedItem,
        context: anchorContext(order, before.selectedItem),
      };
    }
    set({
      selected: section,
      sectionMemory: memory,
      loading: true,
      loadError: null,
      ...(sameSection
        ? {}
        : {
            items: [],
            selectedItem: null,
            selectedKeys: new Set<string>(),
            rangeOrigin: null,
            rangeBase: new Set<string>(),
            scrollRequest: null,
            detail: null,
          }),
    });
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
        if (sameSection) {
          const previousOrder = sortItems(get().items, get().currentSort()).map(itemKey);
          set({ items, loading: false, loadError: null });
          reconcileReloadSelection(set, get, previousOrder);
        } else {
          set({ items, loading: false, loadError: null });
          const currentOrder = sortItems(items, get().currentSort()).map(itemKey);
          const remembered = restore ?? get().sectionMemory[sectionId(section)] ?? null;
          const anchor = remembered === null
            ? (currentOrder[0] ?? null)
            : recoverAnchor(currentOrder, remembered.anchor, remembered.context);
          applyExclusiveSelection(set, anchor, remembered === null ? "nearest" : "center");
        }
      }
    } catch (error) {
      log.error("section items load failed", toErrorFields(error));
      // Only the latest request may blank — a rejection for a section the
      // user already navigated away from must not wipe the live one.
      if (fresh()) {
        set({
          ...(sameSection ? {} : { items: [] }),
          loading: false,
          loadError: "Couldn’t load this section.",
        });
      }
    }
  },

  selectItem: (key, align = "nearest") => {
    set({
      selectedItem: key,
      selectedKeys: key === null ? new Set() : new Set([key]),
      rangeOrigin: key,
      rangeBase: key === null ? new Set<string>() : new Set([key]),
      scrollRequest:
        key === null ? null : requestScroll(key, align),
      detail: null,
    });
    loadAnchorDetail(key);
  },

  // Moves the anchor only. The range origin deliberately stays put, so a
  // Shift+arrow run can reverse and shrink instead of only growing.
  setAnchor: (key) => {
    const selectedKeys = new Set(get().selectedKeys);
    if (key !== null) selectedKeys.add(key);
    set({
      selectedItem: key,
      selectedKeys,
      scrollRequest:
        key === null ? null : requestScroll(key, "nearest"),
      detail: null,
    });
    loadAnchorDetail(key);
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
      rangeOrigin: anchor,
      rangeBase: new Set(next),
      scrollRequest:
        anchor === null ? null : requestScroll(anchor, "nearest"),
      detail: null,
    });
    loadAnchorDetail(anchor);
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
    if (to < 0) return;
    if (from < 0) {
      applyExclusiveSelection(set, key, "nearest");
      return;
    }
    const [start, end] = from <= to ? [from, to] : [to, from];
    const next = new Set(rangeBase);
    for (const k of sortedKeys.slice(start, end + 1)) next.add(k);
    set({
      selectedKeys: next,
      selectedItem: key,
      scrollRequest: requestScroll(key, "nearest"),
      detail: null,
    });
    loadAnchorDetail(key);
  },

  selectAfterFamily: (memberHashes, orderBeforeComparison) => {
    const { items } = get();
    const family = new Set(memberHashes);
    const previous = sortItems(orderBeforeComparison, get().currentSort());
    const lastMember = previous.reduce(
      (last, item, index) => (item.hash !== null && family.has(item.hash) ? index : last),
      -1,
    );
    const live = new Set(items.map(itemKey));
    const after = previous
      .slice(lastMember + 1)
      .map(itemKey)
      .find((key) => live.has(key));
    const before = previous
      .slice(0, Math.max(0, lastMember + 1))
      .reverse()
      .map(itemKey)
      .find((key) => live.has(key));
    get().selectItem(after ?? before ?? null);
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
    const anchorRemapped = selectedItem !== state.selectedItem;
    set({
      items,
      selectedItem,
      selectedKeys: remapSet(state.selectedKeys),
      rangeOrigin: remap(state.rangeOrigin),
      rangeBase: remapSet(state.rangeBase),
      scrollRequest:
        state.scrollRequest?.key === previousHash
          ? { ...state.scrollRequest, key: current }
          : state.scrollRequest,
      ...(anchorRemapped ? { detail: null } : {}),
    });
    if (selectedItem === current) loadAnchorDetail(current);
  },
}));

/** Drops whatever the reload did not bring back, leaving everything that
 * survived exactly where it was. Only reached on a same-section reload; a
 * real section switch clears the selection outright. */
function sectionId(section: SelectedSection): string {
  return `${section.kind}:${section.month}`;
}

function applyExclusiveSelection(
  set: (partial: Partial<ItemsState>) => void,
  anchor: string | null,
  align: "nearest" | "center",
): void {
  set({
    selectedItem: anchor,
    selectedKeys: anchor === null ? new Set() : new Set([anchor]),
    rangeOrigin: anchor,
    rangeBase: anchor === null ? new Set() : new Set([anchor]),
    scrollRequest:
      anchor === null ? null : requestScroll(anchor, align),
    detail: null,
  });
  loadAnchorDetail(anchor);
}

function reconcileReloadSelection(
  set: (partial: Partial<ItemsState>) => void,
  get: () => ItemsState,
  previousOrder: string[],
): void {
  const { items, selectedItem, selectedKeys, rangeOrigin, rangeBase } = get();
  const currentOrder = sortItems(items, get().currentSort()).map(itemKey);
  const alive = new Set(currentOrder);
  const keys = new Set([...selectedKeys].filter((k) => alive.has(k)));
  const context = anchorContext(previousOrder, selectedItem);
  let anchor = selectedItem !== null && alive.has(selectedItem) ? selectedItem : null;
  if (anchor === null && keys.size > 0) {
    anchor = recoverAnchor(currentOrder, selectedItem, context, keys);
  }
  if (anchor === null) {
    anchor = recoverAnchor(currentOrder, selectedItem, context);
    if (anchor !== null) {
      keys.clear();
      keys.add(anchor);
    }
  }
  if (anchor !== null) keys.add(anchor);
  const nextRangeBase = new Set([...rangeBase].filter((k) => alive.has(k)));
  if (keys.size === 1 && anchor !== null) nextRangeBase.add(anchor);
  set({
    selectedItem: anchor,
    selectedKeys: keys,
    rangeOrigin: rangeOrigin !== null && alive.has(rangeOrigin) ? rangeOrigin : anchor,
    rangeBase: nextRangeBase,
    scrollRequest:
      anchor !== selectedItem && anchor !== null
        ? requestScroll(anchor, "center")
        : get().scrollRequest,
    ...(anchor !== selectedItem ? { detail: null } : {}),
  });
  if (anchor !== selectedItem) loadAnchorDetail(anchor);
}

/** One detail query belongs to the item state owner. Persistence and Preview
 * projection observe the resulting state at the application edge. */
function loadAnchorDetail(key: string | null): void {
  if (key === null) return;
  const item = useItemsStore.getState().items.find((i) => itemKey(i) === key);
  if (!item) return;
  const payload = { hash: item.hash, pathId: item.hash === null ? item.pathId : null };
  // Both guards: the key check drops a response for an anchor the user left;
  // the sequence drops the OLDER of two responses for the same anchor.
  const fresh = detailLoad.begin();
  void invoke<ItemDetail>("get_item_detail", payload)
    .then((detail) => {
      if (fresh() && useItemsStore.getState().selectedItem === key) {
        useItemsStore.setState({ detail });
      }
    })
    .catch((error) => {
      log.error("item detail load failed", toErrorFields(error));
      if (fresh() && useItemsStore.getState().selectedItem === key) {
        useItemsStore.setState({ message: "Couldn’t load details for this item." });
      }
    });
}

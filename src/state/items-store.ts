// The selected section, its bounded display window, and explicit selection.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { requestSeq } from "./request-seq";
import { log, toErrorFields } from "../repositories";
import { reportActionFailure } from "./notifications-store";
import {
  DEFAULT_DESC,
  SORT_ORDERS,
  identityFromKey,
  identityKey,
  itemKey,
  replaceDerivedItem,
  type ItemDetail,
  type PositionedSectionIdentity,
  type SectionItem,
  type SectionReconciliation,
  type SectionWindow,
  type SortChoice,
  type SortOrder,
} from "../models/items";
import {
  anchorContextFromPayload,
  anchorContextPayload,
  type AnchorContext,
  type SectionMemory,
} from "../models/mainSelection";

export interface SelectedSection {
  kind: "image" | "video" | "other";
  month: string;
}

const SECTION_WINDOW_LIMIT = 512;

interface ItemsState {
  selected: SelectedSection | null;
  /** Only [windowStart, windowStart + items.length) is retained. */
  items: SectionItem[];
  totalItems: number;
  windowStart: number;
  itemPositions: Map<string, number>;
  reconciliationId: number;
  loading: boolean;
  loadError: string | null;
  selectedItem: string | null;
  selectedKeys: Set<string>;
  selectedPositions: Map<string, number>;
  rangeOrigin: string | null;
  rangeOriginPosition: number | null;
  rangeBase: Set<string>;
  rangeBasePositions: Map<string, number>;
  sectionMemory: Record<string, SectionMemory>;
  currentContext: AnchorContext | null;
  scrollRequest: {
    key: string;
    index: number;
    align: "nearest" | "center";
    id: number;
  } | null;
  detail: ItemDetail | null;
  sortOrders: { media: SortChoice; other: SortChoice };
  currentSort: () => SortChoice;
  message: string | null;
  setSortOrder: (order: SortOrder) => void;
  select: (
    section: SelectedSection,
    restore?: { anchor: string | null; context: AnchorContext | null },
  ) => Promise<void>;
  loadWindow: (start: number, force?: boolean) => Promise<void>;
  selectPosition: (index: number, extend: boolean) => Promise<void>;
  selectIdentity: (key: string) => Promise<void>;
  selectItem: (
    key: string | null,
    align?: "nearest" | "center",
    position?: number,
  ) => void;
  setAnchor: (key: string | null, position?: number) => void;
  toggleItem: (key: string, position?: number) => void;
  rangeSelect: (key: string, position?: number) => Promise<void>;
  refresh: () => Promise<void>;
  applyDerivedItem: (previousHash: string, item: SectionItem) => void;
  selectAfterFamily: (recovery: AnchorContext | null) => Promise<void>;
}

const sectionLoad = requestSeq();
const windowLoad = requestSeq();
const rangeLoad = requestSeq();
const detailLoad = requestSeq();
let scrollRequestId = 0;

function requestScroll(
  key: string,
  index: number,
  align: "nearest" | "center",
) {
  scrollRequestId += 1;
  return { key, index, align, id: scrollRequestId };
}

function positionMap(start: number, items: readonly SectionItem[]): Map<string, number> {
  return new Map(items.map((item, offset) => [itemKey(item), start + offset]));
}

function membersMap(members: readonly PositionedSectionIdentity[]): Map<string, number> {
  return new Map(members.map((member) => [identityKey(member), member.index]));
}

function knownPosition(state: ItemsState, key: string): number | undefined {
  return (
    state.itemPositions.get(key) ??
    (() => {
      const local = state.items.findIndex((item) => itemKey(item) === key);
      return local < 0 ? undefined : state.windowStart + local;
    })() ??
    state.selectedPositions.get(key)
  );
}

function sectionId(section: SelectedSection): string {
  return `${section.kind}:${section.month}`;
}

function sameSort(left: SortChoice, right: SortChoice): boolean {
  return left.order === right.order && left.desc === right.desc;
}

export const useItemsStore = create<ItemsState>((set, get) => ({
  selected: null,
  items: [],
  totalItems: 0,
  windowStart: 0,
  itemPositions: new Map(),
  reconciliationId: 0,
  loading: false,
  loadError: null,
  selectedItem: null,
  selectedKeys: new Set(),
  selectedPositions: new Map(),
  rangeOrigin: null,
  rangeOriginPosition: null,
  rangeBase: new Set(),
  rangeBasePositions: new Map(),
  sectionMemory: {},
  currentContext: null,
  scrollRequest: null,
  detail: null,
  sortOrders: {
    media: SORT_ORDERS.media.defaultChoice,
    other: SORT_ORDERS.other.defaultChoice,
  },
  message: null,

  currentSort: () => {
    const state = get();
    return state.selected?.kind === "other" ? state.sortOrders.other : state.sortOrders.media;
  },

  setSortOrder: (order) => {
    const state = get();
    const lane = state.selected?.kind === "other" ? "other" : "media";
    const current = state.sortOrders[lane];
    const next: SortChoice =
      current.order === order
        ? { order, desc: !current.desc }
        : { order, desc: DEFAULT_DESC[order] };
    set({ sortOrders: { ...state.sortOrders, [lane]: next } });
    void reconcileCurrent(set, get, false, "center");
  },

  select: async (section, restore) => {
    const before = get();
    const sameSection =
      before.selected?.kind === section.kind && before.selected.month === section.month;
    const memory = { ...before.sectionMemory };
    if (!sameSection && before.selected !== null) {
      memory[sectionId(before.selected)] = {
        anchor: before.selectedItem,
        context: before.currentContext,
      };
    }
    const remembered = restore ?? memory[sectionId(section)] ?? null;
    set({
      selected: section,
      sectionMemory: memory,
      loading: true,
      loadError: null,
      ...(!sameSection
        ? {
            items: [],
            totalItems: 0,
            windowStart: 0,
            itemPositions: new Map<string, number>(),
            selectedItem: null,
            selectedKeys: new Set<string>(),
            selectedPositions: new Map<string, number>(),
            rangeOrigin: null,
            rangeOriginPosition: null,
            rangeBase: new Set<string>(),
            rangeBasePositions: new Map<string, number>(),
            currentContext: null,
            scrollRequest: null,
            detail: null,
          }
        : {}),
    });
    await reconcileCurrent(
      set,
      get,
      !sameSection && remembered === null,
      remembered === null ? "nearest" : "center",
      !sameSection ? remembered : undefined,
    );
  },

  loadWindow: async (requestedStart, force = false) => {
    const state = get();
    const section = state.selected;
    if (section === null || state.totalItems === 0) return;
    const sort = state.currentSort();
    const maxStart = Math.max(0, state.totalItems - SECTION_WINDOW_LIMIT);
    const start = Math.min(Math.max(Math.floor(requestedStart), 0), maxStart);
    const requestedEnd = Math.min(state.totalItems, start + SECTION_WINDOW_LIMIT);
    if (!force && start >= state.windowStart && requestedEnd <= state.windowStart + state.items.length) {
      return;
    }
    const fresh = windowLoad.begin();
    try {
      const window = await invoke<SectionWindow>("get_section_window", {
        kind: section.kind,
        month: section.month,
        sort,
        start,
        limit: SECTION_WINDOW_LIMIT,
      });
      const current = get();
      if (
        fresh() &&
        current.selected?.kind === section.kind &&
        current.selected.month === section.month &&
        sameSort(current.currentSort(), sort)
      ) {
        set({
          items: window.items,
          totalItems: window.total,
          windowStart: window.start,
          itemPositions: positionMap(window.start, window.items),
          loadError: null,
        });
      }
    } catch (error) {
      log.error("section window load failed", toErrorFields(error));
      if (fresh()) {
        set({ loadError: "Couldn’t load this part of the section." });
        reportActionFailure(
          "section-window-load-failed",
          "Couldn’t load this part of the section.",
          error,
        );
      }
    }
  },

  selectPosition: async (requestedIndex, extend) => {
    const before = get();
    const total = before.totalItems > 0 ? before.totalItems : before.windowStart + before.items.length;
    if (total === 0) return;
    const index = Math.min(Math.max(Math.floor(requestedIndex), 0), total - 1);
    let entry = before.items
      .map((item, offset) => [itemKey(item), before.windowStart + offset] as const)
      .find((candidate) => candidate[1] === index);
    if (entry === undefined) {
      await get().loadWindow(index - Math.floor(SECTION_WINDOW_LIMIT / 2));
      const state = get();
      entry = state.items
        .map((item, offset) => [itemKey(item), state.windowStart + offset] as const)
        .find((candidate) => candidate[1] === index);
    }
    if (entry === undefined) return;
    if (extend) await get().rangeSelect(entry[0], index);
    else get().selectItem(entry[0], "nearest", index);
  },

  selectIdentity: async (key) => {
    await reconcileCurrent(
      set,
      get,
      false,
      "center",
      { anchor: key, context: null },
      true,
    );
  },

  selectItem: (key, align = "nearest", position) => {
    if (key === null) {
      set({
        selectedItem: null,
        selectedKeys: new Set(),
        selectedPositions: new Map(),
        rangeOrigin: null,
        rangeOriginPosition: null,
        rangeBase: new Set(),
        rangeBasePositions: new Map(),
        currentContext: null,
        scrollRequest: null,
        detail: null,
      });
      return;
    }
    const index = position ?? knownPosition(get(), key);
    if (index === undefined) return;
    const positions = new Map([[key, index]]);
    set({
      selectedItem: key,
      selectedKeys: new Set([key]),
      selectedPositions: positions,
      rangeOrigin: key,
      rangeOriginPosition: index,
      rangeBase: new Set([key]),
      rangeBasePositions: new Map(positions),
      currentContext: contextFromWindow(get(), index),
      scrollRequest: requestScroll(key, index, align),
      detail: null,
    });
    loadAnchorDetail(key);
  },

  setAnchor: (key, position) => {
    if (key === null) {
      set({ selectedItem: null, scrollRequest: null, detail: null, currentContext: null });
      return;
    }
    const index = position ?? knownPosition(get(), key);
    if (index === undefined) return;
    const selectedPositions = new Map(get().selectedPositions).set(key, index);
    set({
      selectedItem: key,
      selectedKeys: new Set(selectedPositions.keys()),
      selectedPositions,
      currentContext: contextFromWindow(get(), index),
      scrollRequest: requestScroll(key, index, "nearest"),
      detail: null,
    });
    loadAnchorDetail(key);
  },

  toggleItem: (key, position) => {
    const state = get();
    const index = position ?? knownPosition(state, key);
    if (index === undefined) return;
    const selectedPositions = new Map(state.selectedPositions);
    const removing = selectedPositions.delete(key);
    if (!removing) selectedPositions.set(key, index);
    const ordered = [...selectedPositions.entries()].sort((left, right) => left[1] - right[1]);
    let anchor = key;
    let anchorIndex = index;
    if (removing && state.selectedItem !== key && state.selectedItem !== null) {
      anchor = state.selectedItem;
      anchorIndex = state.selectedPositions.get(anchor) ?? index;
    } else if (removing) {
      const next = ordered.find((entry) => entry[1] > index);
      const previous = [...ordered].reverse().find((entry) => entry[1] < index);
      const replacement = next ?? previous;
      anchor = replacement?.[0] ?? "";
      anchorIndex = replacement?.[1] ?? 0;
    }
    const selectedItem = anchor === "" ? null : anchor;
    set({
      selectedItem,
      selectedKeys: new Set(selectedPositions.keys()),
      selectedPositions,
      rangeOrigin: selectedItem,
      rangeOriginPosition: selectedItem === null ? null : anchorIndex,
      rangeBase: new Set(selectedPositions.keys()),
      rangeBasePositions: new Map(selectedPositions),
      currentContext: selectedItem === null ? null : contextFromWindow(get(), anchorIndex),
      scrollRequest:
        selectedItem === null ? null : requestScroll(selectedItem, anchorIndex, "nearest"),
      detail: null,
    });
    loadAnchorDetail(selectedItem);
  },

  rangeSelect: async (key, position) => {
    const state = get();
    const section = state.selected;
    const target = position ?? state.itemPositions.get(key);
    const origin = state.rangeOriginPosition ?? state.selectedPositions.get(state.selectedItem ?? "");
    if (section === null || target === undefined || origin === undefined) {
      get().selectItem(key, "nearest", target);
      return;
    }
    const [start, end] = origin <= target ? [origin, target + 1] : [target, origin + 1];
    const fresh = rangeLoad.begin();
    try {
      const members = await invoke<PositionedSectionIdentity[]>("get_section_range", {
        kind: section.kind,
        month: section.month,
        sort: state.currentSort(),
        start,
        end,
      });
      if (!fresh()) return;
      const selectedPositions = new Map(state.rangeBasePositions);
      for (const member of members) selectedPositions.set(identityKey(member), member.index);
      set({
        selectedItem: key,
        selectedKeys: new Set(selectedPositions.keys()),
        selectedPositions,
        currentContext: contextFromWindow(get(), target),
        scrollRequest: requestScroll(key, target, "nearest"),
        detail: null,
      });
      loadAnchorDetail(key);
    } catch (error) {
      log.error("section range selection failed", toErrorFields(error));
      reportActionFailure("section-range-load-failed", "Couldn’t extend the selection.", error);
    }
  },

  refresh: async () => {
    if (get().selected !== null) await reconcileCurrent(set, get, false, "center");
  },

  applyDerivedItem: (previousHash, item) => {
    const state = get();
    const items = replaceDerivedItem(state.items, previousHash, item);
    if (items === state.items || item.hash === null) return;
    const current = item.hash;
    const remap = (key: string | null) => (key === previousHash ? current : key);
    const remapPositions = (positions: Map<string, number>) => {
      const next = new Map(positions);
      const position = next.get(previousHash);
      if (position !== undefined) {
        next.delete(previousHash);
        next.set(current, position);
      }
      return next;
    };
    const selectedPositions = remapPositions(state.selectedPositions);
    const rangeBasePositions = remapPositions(state.rangeBasePositions);
    const selectedItem = remap(state.selectedItem);
    set({
      items,
      itemPositions: positionMap(state.windowStart, items),
      selectedItem,
      selectedKeys: new Set(selectedPositions.keys()),
      selectedPositions,
      rangeOrigin: remap(state.rangeOrigin),
      rangeBase: new Set(rangeBasePositions.keys()),
      rangeBasePositions,
      scrollRequest:
        state.scrollRequest?.key === previousHash
          ? { ...state.scrollRequest, key: current }
          : state.scrollRequest,
      ...(selectedItem !== state.selectedItem ? { detail: null } : {}),
    });
    void get().loadWindow(state.windowStart, true);
    if (selectedItem === current) loadAnchorDetail(current);
  },

  selectAfterFamily: async (recovery) => {
    if (get().selected === null || recovery === null) {
      get().selectItem(null);
      return;
    }
    await reconcileCurrent(
      set,
      get,
      false,
      "center",
      { anchor: "__comparison-family-boundary__", context: recovery },
      true,
    );
  },
}));

async function reconcileCurrent(
  set: (partial: Partial<ItemsState>) => void,
  get: () => ItemsState,
  selectFirst: boolean,
  align: "nearest" | "center",
  remembered?: { anchor: string | null; context: AnchorContext | null } | null,
  replaceSelection = false,
): Promise<void> {
  const before = get();
  const section = before.selected;
  if (section === null) return;
  const sort = before.currentSort();
  const fresh = sectionLoad.begin();
  const selected = replaceSelection ? [] : [...before.selectedKeys].map(identityFromKey);
  const rangeBase = replaceSelection ? [] : [...before.rangeBase].map(identityFromKey);
  const requestedAnchor = remembered === undefined ? before.selectedItem : remembered?.anchor ?? null;
  const inferredContext =
    before.selectedItem === null
      ? null
      : (() => {
          const position = knownPosition(before, before.selectedItem!);
          return position === undefined ? null : contextFromWindow(before, position);
        })();
  const recovery =
    remembered === undefined
      ? (before.currentContext ?? inferredContext)
      : remembered?.context ?? null;
  try {
    const result = await invoke<SectionReconciliation>("reconcile_section", {
      kind: section.kind,
      month: section.month,
      sort,
      selected,
      anchor: requestedAnchor === null ? null : identityFromKey(requestedAnchor),
      rangeOrigin:
        replaceSelection || before.rangeOrigin === null
          ? null
          : identityFromKey(before.rangeOrigin),
      rangeBase,
      recovery: anchorContextPayload(recovery),
      selectFirst,
      limit: SECTION_WINDOW_LIMIT,
    });
    const current = get();
    if (
      !fresh() ||
      current.selected?.kind !== section.kind ||
      current.selected.month !== section.month ||
      !sameSort(current.currentSort(), sort)
    ) return;
    const selectedPositions = membersMap(result.selected);
    const anchor = result.anchor === null ? null : identityKey(result.anchor);
    if (anchor !== null && result.anchor !== null) selectedPositions.set(anchor, result.anchor.index);
    const rangeBasePositions = membersMap(result.rangeBase);
    if (rangeBasePositions.size === 0 && anchor !== null && selectedPositions.size === 1) {
      rangeBasePositions.set(anchor, result.anchor!.index);
    }
    const rangeOrigin = result.rangeOrigin === null ? anchor : identityKey(result.rangeOrigin);
    const rangeOriginPosition = result.rangeOrigin?.index ?? result.anchor?.index ?? null;
    set({
      items: result.window.items,
      totalItems: result.window.total,
      windowStart: result.window.start,
      itemPositions: positionMap(result.window.start, result.window.items),
      reconciliationId: current.reconciliationId + 1,
      loading: false,
      loadError: null,
      selectedItem: anchor,
      selectedKeys: new Set(selectedPositions.keys()),
      selectedPositions,
      rangeOrigin,
      rangeOriginPosition,
      rangeBase: new Set(rangeBasePositions.keys()),
      rangeBasePositions,
      currentContext: anchorContextFromPayload(result.context),
      scrollRequest:
        anchor === null || result.anchor === null
          ? null
          : requestScroll(anchor, result.anchor.index, align),
      ...(anchor !== before.selectedItem ? { detail: null } : {}),
    });
    if (anchor !== before.selectedItem) loadAnchorDetail(anchor);
  } catch (error) {
    log.error("section reconciliation failed", toErrorFields(error));
    if (fresh()) {
      set({ loading: false, loadError: "Couldn’t load this section." });
      reportActionFailure("section-items-load-failed", "Couldn’t load this section.", error);
    }
  }
}

function contextFromWindow(state: ItemsState, index: number): AnchorContext {
  const local = index - state.windowStart;
  const keys = state.items.map(itemKey);
  if (local < 0 || local >= keys.length) {
    return { index, before: [], after: [] };
  }
  return {
    index,
    before: keys.slice(Math.max(0, local - 64), local).reverse(),
    after: keys.slice(local + 1, local + 65),
  };
}

function loadAnchorDetail(key: string | null): void {
  if (key === null) return;
  const payload = identityFromKey(key);
  const fresh = detailLoad.begin();
  void invoke<ItemDetail>("get_item_detail", {
    hash: payload.hash,
    pathId: payload.hash === null ? payload.pathId : null,
  })
    .then((detail) => {
      if (fresh() && useItemsStore.getState().selectedItem === key) {
        useItemsStore.setState({ detail });
      }
    })
    .catch((error) => {
      log.error("item detail load failed", toErrorFields(error));
      if (fresh() && useItemsStore.getState().selectedItem === key) {
        useItemsStore.setState({ message: "Couldn’t load details for this item." });
        reportActionFailure("item-detail-load-failed", "Couldn’t load details for this item.", error);
      }
    });
}

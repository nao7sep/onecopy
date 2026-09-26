// @vitest-environment happy-dom

// R5.1 D10: restart restores the last section and anchor. `useAppBootstrapAndRestore`
// is the one place a fresh app run turns `lastSection` / `lastItem` /
// `lastItemContext` (persisted by `installItemWorkflow`, workflows/items.ts:58-70)
// back into a `select()` call; no test in the repo exercised that round trip
// before this file. This drives the hook against the real `items-store`
// (mocked only at the backend boundary, as `tests/state/items-store.test.ts`
// does) rather than spying on the store's own method, so the assertion is on
// the actual restored selection, not on how the hook happens to call it.

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useAppBootstrapAndRestore } from "../../src/hooks/useAppBootstrapAndRestore";
import { useItemsStore } from "../../src/state/items-store";
import { usePreviewStore } from "../../src/state/preview-store";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import type { LoadedAppData } from "../../src/repositories";
import type { SectionCounts } from "../../src/models/sections";
import { mockCommands, mockSectionItems, resetTauriMocks } from "../mocks/tauri";

function item(pathId: number): SectionItem {
  return {
    hash: `h${pathId}`,
    pathId,
    fileName: `IMG_${String(pathId).padStart(4, "0")}.jpg`,
    resolvedUtcMs: pathId * 1000,
    copyCount: 1,
    width: 100,
    height: 100,
    hasThumb: true,
    similarGroupId: null,
    similarCount: 0,
    sharpness: null,
    faceScore: null,
    byteSize: 1000,
    hasCompanions: false,
    durationMs: null,
    dirPaths: ["/photos"],
    derivedWork: EMPTY_ITEM_WORK,
  };
}

const counts: SectionCounts = {
  images: [{ month: "2026-01", count: 3 }],
  videos: [],
  others: [],
};

function appData(state: Record<string, unknown>): LoadedAppData {
  return {
    config: {},
    state,
    dataRoot: "/test",
    debugEnabled: false,
    quarantines: [],
  } as unknown as LoadedAppData;
}

function resetItemsStore(): void {
  useItemsStore.setState({
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
      media: { order: "time", desc: false },
      other: { order: "name", desc: false },
    },
  });
}

beforeEach(() => {
  resetTauriMocks();
  resetItemsStore();
  mockCommands({
    activity_record: () => null,
    record_recent_notification: () => ({}),
    patch_state: () => ({}),
    get_item_detail: () => ({ fileName: "item", kind: "image" }),
    get_section_counts: () => [],
  });
  mockSectionItems(() => [item(1), item(2), item(42)]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("restores the last section and anchor exclusively once appData and counts are both ready", async () => {
  const data = appData({
    lastSection: { kind: "image", month: "2026-01" },
    lastItem: "h42",
    lastItemContext: null,
  });

  const { rerender } = renderHook(
    (props: { appData: LoadedAppData | null; counts: SectionCounts | null }) =>
      useAppBootstrapAndRestore({ ...props, restorePaneIntents: () => {} }),
    { initialProps: { appData: null, counts: null } },
  );
  // Neither piece alone is enough to restore -- the effect requires both.
  rerender({ appData: data, counts: null });
  expect(useItemsStore.getState().selected).toBeNull();
  rerender({ appData: null, counts });
  expect(useItemsStore.getState().selected).toBeNull();

  await act(async () => {
    rerender({ appData: data, counts });
  });

  const restored = useItemsStore.getState();
  expect(restored.selected).toEqual({ kind: "image", month: "2026-01" });
  expect(restored.selectedItem).toBe("h42");
  expect([...restored.selectedKeys]).toEqual(["h42"]);

  // A later re-render (e.g. counts refreshing) never restores a second time:
  // moving the anchor manually and re-rendering must leave it untouched.
  useItemsStore.getState().selectItem("h1", "nearest", 0);
  await act(async () => {
    rerender({ appData: data, counts });
  });
  expect(useItemsStore.getState().selectedItem).toBe("h1");
});

it("restores nothing when the saved section no longer exists in the fresh counts", async () => {
  const data = appData({ lastSection: { kind: "image", month: "2019-06" }, lastItem: "gone" });

  await act(async () => {
    renderHook(() =>
      useAppBootstrapAndRestore({ appData: data, counts, restorePaneIntents: () => {} }),
    );
  });

  expect(useItemsStore.getState().selected).toBeNull();
});

it("restores Preview's follow/placement alongside the section on the same restart", async () => {
  const restoreFollow = vi.spyOn(usePreviewStore.getState(), "restoreFollow");
  const data = appData({
    previewFollow: true,
    previewPlacement: "window",
    lastSection: { kind: "image", month: "2026-01" },
    lastItem: "h42",
  });

  await act(async () => {
    renderHook(() =>
      useAppBootstrapAndRestore({ appData: data, counts, restorePaneIntents: () => {} }),
    );
  });

  expect(restoreFollow).toHaveBeenCalledWith(true, "window");
});

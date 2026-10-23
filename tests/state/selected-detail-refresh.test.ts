// @vitest-environment happy-dom

import { EMPTY_ITEM_WORK } from "../helpers/itemWork";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { useItemsStore } from "../../src/state/items-store";
import { usePreviewStore } from "../../src/state/preview-store";
import { installItemWorkflow } from "../../src/workflows/items";
import { type ItemDetail, type SectionItem } from "../../src/models/items";
import { invokeCalls, mockCommands, mockSectionItems, resetTauriMocks } from "../mocks/tauri";

beforeAll(() => installItemWorkflow());

beforeEach(() => {
  resetTauriMocks();
  mockCommands({ activity_record: () => null, patch_state: () => ({}), log_event: () => null });
  useItemsStore.setState({
    selected: null, selectedItem: null, selectedKeys: new Set(), selectedPositions: new Map(),
    items: [], totalItems: 0, windowStart: 0, itemPositions: new Map(), orderToken: null,
    rangeOrigin: null, rangeOriginPosition: null, rangeBase: new Set(), rangeBasePositions: new Map(),
    sectionMemory: {}, currentContext: null, scrollRequest: null, detail: null, loadError: null,
    sortOrders: { media: { order: "time", desc: false }, other: { order: "name", desc: false } },
  });
  usePreviewStore.setState({ follow: true, placement: "split", current: null });
});

describe("selected detail follows source-copy changes into Main and Preview", () => {
  it("adds and removes duplicate paths while preserving the surviving identity and marks", async () => {
    let copyPaths = ["/photos/a.jpg"];
    const row = (): SectionItem => ({
      hash: "ha", pathId: 1, fileName: "a.jpg", resolvedUtcMs: 1000, copyCount: copyPaths.length,
      width: 100, height: 100, hasThumb: true, similarGroupId: null, similarCount: 0,
      sharpness: null, faceScore: null, byteSize: 1000, hasCompanions: false, durationMs: null,
      dirPaths: copyPaths.map((path) => path.slice(0, path.lastIndexOf("/"))), derivedWork: EMPTY_ITEM_WORK,
    });
    const detail = (): ItemDetail => ({
      fileName: "a.jpg", kind: "image", byteSize: 1000, width: 100, height: 100, durationMs: null,
      dateState: "dated", resolvedUtcMs: 1000, resolvedSource: "metadata", dateOnly: false,
      copyPaths: [...copyPaths], copyCount: copyPaths.length, companionPaths: [], stripFrames: null,
    });
    mockSectionItems(() => [row()]);
    mockCommands({ get_item_detail: () => detail() });
    await useItemsStore.getState().select({ kind: "image", month: "2026-01" });
    await vi.waitFor(() => expect(usePreviewStore.getState().current?.detail?.copyPaths).toEqual(copyPaths));
    const marks = useItemsStore.getState().selectedKeys;

    for (const paths of [["/photos/a.jpg", "/backup/a.jpg"], ["/backup/a.jpg"]]) {
      copyPaths = paths;
      useItemsStore.setState({ scrollRequest: null });
      await useItemsStore.getState().refresh();
      await vi.waitFor(() => expect(usePreviewStore.getState().current?.detail?.copyPaths).toEqual(paths));
      expect(useItemsStore.getState().detail?.copyPaths).toEqual(paths);
      expect(useItemsStore.getState().detail?.copyCount).toBe(paths.length);
      expect(usePreviewStore.getState().current?.hash).toBe("ha");
      expect(usePreviewStore.getState().current?.detail?.copyCount).toBe(paths.length);
      expect(useItemsStore.getState().selectedKeys).toEqual(marks);
      expect(useItemsStore.getState().selectedItem).toBe("ha");
      expect(useItemsStore.getState().scrollRequest).toBeNull();
    }
    invokeCalls.length = 0;
    await useItemsStore.getState().refreshWindow();
    expect(invokeCalls.filter((call) => call.command === "get_item_detail")).toHaveLength(0);
  });
});

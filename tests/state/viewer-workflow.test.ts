import { beforeEach, describe, expect, it } from "vitest";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { useItemsStore } from "../../src/state/items-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import {
  handleViewerKey,
  moveViewer,
  openViewerFromMain,
} from "../../src/workflows/quick-view";
import {
  WebviewWindow,
  invokeCalls,
  mockCommand,
  resetTauriMocks,
  setCurrentMonitor,
} from "../mocks/tauri";

function item(key: string, pathId: number): SectionItem {
  return {
    hash: key,
    pathId,
    fileName: `${key}.jpg`,
    resolvedUtcMs: pathId,
    copyCount: 1,
    width: 10,
    height: 10,
    hasThumb: true,
    similarGroupId: null,
    sharpness: null,
    faceScore: null,
    byteSize: pathId,
    hasCompanions: false,
    durationMs: null,
    dirPaths: [],
    derivedWork: EMPTY_ITEM_WORK,
  };
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommand("set_window_simple_fullscreen", () => null);
  useQuickViewStore.setState({ session: null, pendingDelete: null });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: [item("c", 3), item("a", 1), item("b", 2)],
    selectedItem: "b",
    selectedKeys: new Set(["b"]),
    sortOrders: {
      media: { order: "name", desc: false },
      other: { order: "name", desc: false },
    },
    detail: null,
  });
});

describe("viewer workflow", () => {
  it("freezes displayed order and makes whole-section navigation exclusive", () => {
    expect(openViewerFromMain("quick")).toBe(true);
    expect(useQuickViewStore.getState().session?.members.map((member) => member.key)).toEqual([
      "a",
      "b",
      "c",
    ]);

    moveViewer("next");

    expect(useItemsStore.getState().selectedItem).toBe("c");
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["c"]);
  });

  it("preserves a frozen selected subset while moving only its anchor", () => {
    useItemsStore.setState({ selectedItem: "c", selectedKeys: new Set(["c", "a"]) });
    expect(openViewerFromMain("quick")).toBe(true);

    moveViewer("previous");

    expect(useItemsStore.getState().selectedItem).toBe("a");
    expect(useItemsStore.getState().selectedKeys).toEqual(new Set(["c", "a"]));
  });

  it("reuses one borderless fullscreen window and leaves presentation before hiding", async () => {
    const monitor = {
      position: { x: 100, y: 200 },
      size: { width: 1920, height: 1080 },
      workArea: { position: { x: 100, y: 200 }, size: { width: 1920, height: 1040 } },
      scaleFactor: 2,
      name: "display",
    };
    setCurrentMonitor(monitor);
    const viewer = new WebviewWindow("viewer");

    expect(openViewerFromMain("fullscreen")).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(viewer.setPosition).toHaveBeenCalledWith({ x: 100, y: 200 });
    expect(viewer.setSize).toHaveBeenCalledWith({ width: 1920, height: 1080 });
    expect(viewer.setAlwaysOnTop).toHaveBeenCalledWith(true);
    expect(invokeCalls).toContainEqual({
      command: "set_window_simple_fullscreen",
      args: { label: "viewer", enable: true },
    });

    await handleViewerKey({ key: "f" });

    expect(invokeCalls).toContainEqual({
      command: "set_window_simple_fullscreen",
      args: { label: "viewer", enable: false },
    });
    expect(viewer.setAlwaysOnTop).toHaveBeenLastCalledWith(false);
    expect(viewer.hide).toHaveBeenCalled();
    expect(useQuickViewStore.getState().session).toBeNull();
  });
});

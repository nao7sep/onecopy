import { beforeEach, describe, expect, it } from "vitest";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { useItemsStore } from "../../src/state/items-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import {
  handleViewerKey,
  moveViewer,
  openViewerFromMain,
  viewerBroadcast,
} from "../../src/workflows/quick-view";
import {
  WebviewWindow,
  invokeCalls,
  mockCommand,
  resetTauriMocks,
  setCurrentMonitor,
} from "../mocks/tauri";

let sequence: Array<SectionItem> = [];
let sequenceIndex = 0;

function sequenceSnapshot() {
  const current = sequence[sequenceIndex]!;
  return {
    token: "viewer-token",
    member: { hash: current.hash, pathId: current.pathId },
    item: current,
    detail: {
      fileName: current.fileName, kind: "image", byteSize: current.byteSize,
      width: current.width, height: current.height, durationMs: null,
      dateState: "dated", resolvedUtcMs: current.resolvedUtcMs,
      resolvedSource: "metadata", dateOnly: false, copyPaths: [],
      companionPaths: [], stripFrames: null,
    },
    index: sequenceIndex,
    length: sequence.length,
    sectionIndex: current.pathId - 1,
    scope: sequence.length === 3 ? "section" : "selection",
  };
}

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
  mockCommand("set_window_fullscreen", () => null);
  mockCommand("viewer_sequence_start", ({ selected }) => {
    const picked = selected as Array<{ hash: string; index: number }>;
    sequence = picked.length === 1
      ? [item("a", 1), item("b", 2), item("c", 3)]
      : [...picked]
          .sort((left, right) => left.index - right.index)
          .map((member) => item(member.hash, member.index + 1));
    sequenceIndex = Math.max(0, sequence.findIndex((entry) => entry.hash === "b" || entry.hash === "c"));
    return sequenceSnapshot();
  });
  mockCommand("viewer_sequence_move", ({ movement }) => {
    if (movement === "next") sequenceIndex = Math.min(sequence.length - 1, sequenceIndex + 1);
    if (movement === "previous") sequenceIndex = Math.max(0, sequenceIndex - 1);
    return sequenceSnapshot();
  });
  mockCommand("viewer_sequence_close", () => null);
  mockCommand("log_event", () => null);
  mockCommand("record_recent_notification", () => ({}));
  useQuickViewStore.setState({ session: null, pendingDelete: null, failure: null });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: [item("c", 3), item("a", 1), item("b", 2)],
    selectedItem: "b",
    selectedKeys: new Set(["b"]),
    selectedPositions: new Map([["b", 1]]),
    totalItems: 3,
    windowStart: 0,
    itemPositions: new Map([
      ["c", 2],
      ["a", 0],
      ["b", 1],
    ]),
    sortOrders: {
      media: { order: "name", desc: false },
      other: { order: "name", desc: false },
    },
    detail: null,
  });
});

describe("viewer workflow", () => {
  it("owns its identity, detail and content commands independently of Main", async () => {
    expect(openViewerFromMain("quick")).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    const frozen = viewerBroadcast();

    useItemsStore.setState({ selectedItem: "unrelated", detail: null, selected: { kind: "other", month: "undated" } });

    expect(viewerBroadcast()).toEqual(frozen);
    expect(frozen.item?.fileName).toBe("b.jpg");
    expect(frozen.detail?.fileName).toBe("b.jpg");
    await handleViewerKey({ key: "PageDown" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(viewerBroadcast().item?.fileName).toBe("c.jpg");
    expect(viewerBroadcast().detail?.fileName).toBe("c.jpg");
  });

  it("freezes displayed order and makes whole-section navigation exclusive", async () => {
    expect(openViewerFromMain("quick")).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useQuickViewStore.getState().session).toMatchObject({ index: 1, length: 3 });

    moveViewer("next");
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(useItemsStore.getState().selectedItem).toBe("c");
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["c"]);
  });

  it("preserves a frozen selected subset while moving only its anchor", async () => {
    useItemsStore.setState({
      selectedItem: "c",
      selectedKeys: new Set(["c", "a"]),
      selectedPositions: new Map([
        ["c", 2],
        ["a", 0],
      ]),
    });
    expect(openViewerFromMain("quick")).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));

    moveViewer("previous");
    await new Promise((resolve) => setTimeout(resolve, 0));

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
      command: "set_window_fullscreen",
      args: { label: "viewer", enable: true },
    });

    await handleViewerKey({ key: "f" });

    expect(invokeCalls).toContainEqual({
      command: "set_window_fullscreen",
      args: { label: "viewer", enable: false },
    });
    expect(viewer.setAlwaysOnTop).toHaveBeenLastCalledWith(false);
    expect(viewer.hide).toHaveBeenCalled();
    expect(useQuickViewStore.getState().session).toBeNull();
  });

  it("keeps navigation failure on the viewer while recording only Recent history", async () => {
    expect(openViewerFromMain("quick")).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    mockCommand("viewer_sequence_move", () =>
      Promise.reject(new Error("sequence unavailable")),
    );

    moveViewer("next");
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(useQuickViewStore.getState().failure).toBe(
      "Couldn’t move in the viewer.",
    );
    expect(
      invokeCalls.some((call) => call.command === "record_recent_notification"),
    ).toBe(true);
    expect(
      invokeCalls.some((call) => call.command === "publish_notification"),
    ).toBe(false);
  });

  it("does not repeat deletion into the recovered next item", async () => {
    expect(openViewerFromMain("quick")).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));

    await handleViewerKey({ key: "Delete", repeat: true });

    expect(useQuickViewStore.getState().pendingDelete).toBeNull();
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
    expect(useQuickViewStore.getState().session?.item?.hash).toBe("b");
  });

  it("does not let a held entry key or a pending confirmation change presentation", async () => {
    expect(openViewerFromMain("quick")).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    for (const key of [" ", "f", "Escape", "Enter"]) {
      await handleViewerKey({ key, repeat: true });
      expect(useQuickViewStore.getState().session?.presentation).toBe("quick");
    }
    useQuickViewStore.setState({ pendingDelete: "permanent" });
    for (const key of [" ", "f", "Escape", "ArrowRight"]) await handleViewerKey({ key });
    expect(useQuickViewStore.getState().session?.item.hash).toBe("b");
    expect(useQuickViewStore.getState().session?.presentation).toBe("quick");
  });
});

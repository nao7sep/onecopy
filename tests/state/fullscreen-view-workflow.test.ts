// @vitest-environment happy-dom
//
// The workflows under test record failures through the notifications store,
// which renders the sentence in the language the document declares, so these
// specs need a document even though the subject is pure workflow logic.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { useItemsStore } from "../../src/state/items-store";
import { useFullscreenViewStore } from "../../src/state/fullscreen-view-store";
import type { ViewerSequenceSnapshot } from "../../src/models/viewerSession";
import { revealInMain } from "../../src/workflows/reveal-in-main";
import {
  confirmFullscreenViewDelete,
  handleFullscreenViewKey,
  moveFullscreenView,
  requestFullscreenViewDelete,
  openFullscreenView,
  fullscreenViewBroadcast,
  closeFullscreenView,
  installFullscreenViewWorkflow,
} from "../../src/workflows/fullscreen-view";
import {
  WebviewWindow,
  fireEvent,
  invoke,
  invokeCalls,
  mockCommand,
  mockSectionItems,
  mockFullscreenDisplay,
  resetTauriMocks,
  setCurrentMonitor,
  setFocus,
} from "../mocks/tauri";
import { inEnglish } from "../helpers/i18n";
import { currentMainFeedback, useMainFeedbackStore } from "../../src/state/main-feedback-store";

let sequence: Array<SectionItem> = [];
let sequenceIndex = 0;

function sequenceSnapshot() {
  const current = sequence[sequenceIndex]!;
  return {
    token: "fullscreen-view-token",
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
  mockFullscreenDisplay();
  mockSectionItems(({ kind }) => kind === "image"
    ? [item("a", 1), item("b", 2), item("c", 3)] : [item("unrelated", 99)]);
  mockCommand("get_item_section", () => ({ kind: "image", month: "2026-01" }));
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
  useFullscreenViewStore.setState({ session: null, pendingDelete: null, failure: null });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: [item("c", 3), item("a", 1), item("b", 2)],
    selectedItem: "b",
    selectedKeys: new Set(["b"]),
    selectedPositions: new Map([["b", 1]]),
    totalItems: 3,
    windowStart: 0,
    reconciliationId: 0,
    loading: false,
    loadError: null,
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

describe("fullscreen view workflow", () => {
  it("reveals a diagnostic target out of the fullscreen view without replacing its new Main selection on close", async () => {
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    mockCommand("resolve_library_path", () => ({
      identity: { hash: "a", pathId: 1 }, section: { kind: "image", month: "2026-01" },
    }));
    let modalClosed = false;
    expect(await revealInMain("/fixture/a.jpg", () => true, () => { modalClosed = true; })).toBe("revealed");
    expect(modalClosed).toBe(true);
    expect(useFullscreenViewStore.getState().session).toBeNull();
    expect(useItemsStore.getState().selectedItem).toBe("a");
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["a"]);
    expect(invokeCalls.filter((call) => call.command === "reconcile_section")).toHaveLength(1);
    expect(invokeCalls.some((call) => call.command === "viewer_sequence_close")).toBe(true);
  });

  it("owns its identity, detail and content commands independently of Main", async () => {
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    const frozen = fullscreenViewBroadcast();

    useItemsStore.setState({ selectedItem: "unrelated", detail: null, selected: { kind: "other", month: "undated" } });

    expect(fullscreenViewBroadcast()).toEqual(frozen);
    expect(frozen.item?.fileName).toBe("b.jpg");
    expect(frozen.detail?.fileName).toBe("b.jpg");
    await handleFullscreenViewKey({ key: "PageDown" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(fullscreenViewBroadcast().item?.fileName).toBe("c.jpg");
    expect(fullscreenViewBroadcast().detail?.fileName).toBe("c.jpg");
    expect(useItemsStore.getState().selected).toEqual({ kind: "image", month: "2026-01" });
    expect(useItemsStore.getState().selectedItem).toBe("c");
  });

  it("freezes displayed order and makes whole-section navigation exclusive", async () => {
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useFullscreenViewStore.getState().session).toMatchObject({ index: 1, length: 3 });

    moveFullscreenView("next");
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
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));

    moveFullscreenView("previous");
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(useItemsStore.getState().selectedItem).toBe("a");
    expect(useItemsStore.getState().selectedKeys).toEqual(new Set(["c", "a"]));
  });

  it("maps frozen navigation into Main's new sort without refreezing or reusing its old ordinal", async () => {
    openFullscreenView();
    await new Promise((resolve) => setTimeout(resolve, 0));
    useItemsStore.getState().setSortOrder("name");
    await new Promise((resolve) => setTimeout(resolve, 0));
    moveFullscreenView("next");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useFullscreenViewStore.getState().session?.index).toBe(2);
    expect(useItemsStore.getState().selectedItem).toBe("c");
    expect(useItemsStore.getState().scrollRequest?.index).toBe(0);
    const reads = invokeCalls.filter((call) => call.command === "reconcile_section").length;
    moveFullscreenView("previous");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useItemsStore.getState().selectedItem).toBe("b");
    expect(invokeCalls.filter((call) => call.command === "reconcile_section")).toHaveLength(reads);
    expect(invokeCalls.filter((call) => call.command === "viewer_sequence_start")).toHaveLength(1);
  });

  it("recovers the frozen subset after Main leaves its section", async () => {
    useItemsStore.setState({ selectedItem: "c", selectedKeys: new Set(["a", "c"]),
      selectedPositions: new Map([["a", 0], ["c", 2]]) });
    openFullscreenView();
    await new Promise((resolve) => setTimeout(resolve, 0));
    await useItemsStore.getState().select({ kind: "other", month: "undated" });
    moveFullscreenView("previous");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useItemsStore.getState().selectedKeys).toEqual(new Set(["a", "c"]));
    expect(useItemsStore.getState().selectedItem).toBe("a");
    expect(useFullscreenViewStore.getState().session?.length).toBe(2);
  });

  it("does not redirect Main if a newer user intent wins the section lookup", async () => {
    openFullscreenView();
    await new Promise((resolve) => setTimeout(resolve, 0));
    await useItemsStore.getState().select({ kind: "other", month: "undated" });
    let release!: (value: { kind: string; month: string }) => void;
    mockCommand("get_item_section", () => new Promise((resolve) => { release = resolve; }));
    moveFullscreenView("next");
    await new Promise((resolve) => setTimeout(resolve, 0));
    useItemsStore.getState().selectItem("unrelated", "nearest", 0);
    release({ kind: "image", month: "2026-01" });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useItemsStore.getState().selected?.kind).toBe("other");
    expect(useItemsStore.getState().selectedItem).toBe("unrelated");
  });

  it("lets an admitted Main restore finish when the viewer closes", async () => {
    openFullscreenView();
    await new Promise((resolve) => setTimeout(resolve, 0));
    await useItemsStore.getState().select({ kind: "other", month: "undated" });
    let release!: () => void;
    mockCommand("reconcile_section", () => new Promise((resolve) => { release = () => resolve({
      anchor: { hash: "c", pathId: 3, index: 2 }, selected: [{ hash: "c", pathId: 3, index: 2 }],
      rangeOrigin: null, rangeBase: [], context: null,
      window: { start: 0, total: 3, items: [item("a", 1), item("b", 2), item("c", 3)] },
    }); }));
    moveFullscreenView("next");
    await new Promise((resolve) => setTimeout(resolve, 0));
    await closeFullscreenView();
    release();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useFullscreenViewStore.getState().session).toBeNull();
    expect(useItemsStore.getState().loading).toBe(false);
    expect(useItemsStore.getState().selectedItem).toBe("c");
  });

  it("reports a Main lookup failure without misreporting the successful viewer move", async () => {
    openFullscreenView();
    await new Promise((resolve) => setTimeout(resolve, 0));
    await useItemsStore.getState().select({ kind: "other", month: "undated" });
    mockCommand("get_item_section", () => { throw new Error("database unavailable"); });
    moveFullscreenView("next");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(fullscreenViewBroadcast().item?.fileName).toBe("c.jpg");
    expect(inEnglish(useFullscreenViewStore.getState().failure)).toBe(
      "Couldn’t locate this item in Main.",
    );
    expect(useItemsStore.getState().selected?.kind).toBe("other");
  });

  it("does not invent a Main section for a no-longer-available viewer item", async () => {
    openFullscreenView();
    await new Promise((resolve) => setTimeout(resolve, 0));
    await useItemsStore.getState().select({ kind: "other", month: "undated" });
    mockCommand("get_item_section", () => null);
    moveFullscreenView("next");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(inEnglish(useFullscreenViewStore.getState().failure)).toBe(
      "This item is no longer available in Main.",
    );
    expect(useItemsStore.getState().selected?.kind).toBe("other");
  });

  it("reuses one borderless window, fullscreen on Main's display, and leaves fullscreen before hiding", async () => {
    const monitor = {
      position: { x: 100, y: 200 },
      size: { width: 1920, height: 1080 },
      workArea: { position: { x: 100, y: 200 }, size: { width: 1920, height: 1040 } },
      scaleFactor: 2,
      name: "display",
    };
    setCurrentMonitor(monitor);
    const viewer = new WebviewWindow("fullscreen-view");

    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(viewer.setPosition).toHaveBeenCalledWith({ x: 100, y: 200 });
    expect(viewer.setSize).toHaveBeenCalledWith({ width: 1920, height: 1080 });
    expect(invokeCalls).toContainEqual({
      command: "set_window_fullscreen",
      args: { label: "fullscreen-view", enable: true },
    });
    // Focused only while OneCopy is still active once the window is up.
    expect(invokeCalls).toContainEqual({
      command: "focus_window_while_active",
      args: { label: "fullscreen-view" },
    });
    expect(viewer.setFocus).toHaveBeenCalledOnce();
    // Shown on that display first, so fullscreen takes that display's frame.
    expect(viewer.show.mock.invocationCallOrder[0]).toBeLessThan(
      invoke.mock.invocationCallOrder[invokeCalls.findIndex((call) => call.command === "set_window_fullscreen")],
    );

    await handleFullscreenViewKey({ key: "Escape" });

    expect(invokeCalls).toContainEqual({
      command: "set_window_fullscreen",
      args: { label: "fullscreen-view", enable: false },
    });
    expect(viewer.hide).toHaveBeenCalled();
    expect(viewer.setAlwaysOnTop).not.toHaveBeenCalled();
    expect(useFullscreenViewStore.getState().session).toBeNull();
  });

  it.each([" ", "Escape"])("closes on %j and gives the keyboard back to Main's list", async (key) => {
    const grid = document.createElement("div");
    grid.id = "main-item-area";
    grid.tabIndex = 0;
    document.body.appendChild(grid);
    setFocus.mockClear();
    setCurrentMonitor({
      position: { x: 0, y: 0 },
      size: { width: 1920, height: 1080 },
      workArea: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1040 } },
      scaleFactor: 1,
      name: "display",
    });
    new WebviewWindow("fullscreen-view");

    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));

    await handleFullscreenViewKey({ key });

    expect(useFullscreenViewStore.getState().session).toBeNull();
    expect(setFocus).toHaveBeenCalled();
    expect(document.activeElement).toBe(grid);
    grid.remove();
  });

  it("closes when OneCopy stops being the active app, leaving focus with the other app", async () => {
    const grid = document.createElement("div");
    grid.id = "main-item-area";
    grid.tabIndex = 0;
    document.body.appendChild(grid);
    await installFullscreenViewWorkflow();
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    setFocus.mockClear();

    fireEvent("app://activation", true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useFullscreenViewStore.getState().session).not.toBeNull();

    fireEvent("app://activation", false);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useFullscreenViewStore.getState().session).toBeNull();
    expect(setFocus).not.toHaveBeenCalled();
    // Returning to OneCopy finds the keyboard on Main's list.
    expect(document.activeElement).toBe(grid);
    grid.remove();
  });

  // Alt+F4 or Close Window on the fullscreen view's window: Rust keeps the
  // window, and the session closes as on Escape, so Main takes keys again.
  it("closes the session when the system asks to close its window", async () => {
    const viewer = new WebviewWindow("fullscreen-view");
    await installFullscreenViewWorkflow();
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useFullscreenViewStore.getState().session).not.toBeNull();

    fireEvent("fullscreen-view://close-requested");
    await vi.waitFor(() => expect(viewer.hide).toHaveBeenCalled());

    expect(useFullscreenViewStore.getState().session).toBeNull();
    expect(invokeCalls.some((call) => call.command === "viewer_sequence_close")).toBe(true);
    expect(viewer.close).not.toHaveBeenCalled();
    expect(viewer.destroy).not.toHaveBeenCalled();
  });

  it("closes the session and reports in Main when its window cannot open", async () => {
    setCurrentMonitor({
      position: { x: 0, y: 0 },
      size: { width: 1920, height: 1080 },
      workArea: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1040 } },
      scaleFactor: 1,
      name: "display",
    });
    new WebviewWindow("fullscreen-view");
    mockCommand("set_window_fullscreen", ({ enable }) => {
      if (enable === true) throw new Error("window unavailable");
      return null;
    });

    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(useFullscreenViewStore.getState().session).toBeNull();
    expect(invokeCalls.some((call) => call.command === "viewer_sequence_close")).toBe(true);
    expect(inEnglish(currentMainFeedback(useMainFeedbackStore.getState())?.text ?? null)).toBe(
      "Couldn’t show the fullscreen view.",
    );
  });

  it("keeps navigation failure on the viewer while recording only Recent history", async () => {
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    mockCommand("viewer_sequence_move", () =>
      Promise.reject(new Error("sequence unavailable")),
    );

    moveFullscreenView("next");
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(inEnglish(useFullscreenViewStore.getState().failure)).toBe(
      "Couldn’t move in the fullscreen view.",
    );
    expect(
      invokeCalls.some((call) => call.command === "record_recent_notification"),
    ).toBe(true);
    expect(
      invokeCalls.some((call) => call.command === "publish_notification"),
    ).toBe(false);
  });

  it("does not repeat deletion into the recovered next item", async () => {
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));

    await handleFullscreenViewKey({ key: "Delete", repeat: true });

    expect(useFullscreenViewStore.getState().pendingDelete).toBeNull();
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
    expect(useFullscreenViewStore.getState().session?.item?.hash).toBe("b");
  });

  it("does not let a held key or a pending confirmation close the view", async () => {
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    for (const key of [" ", "Escape", "Enter"]) {
      await handleFullscreenViewKey({ key, repeat: true });
      expect(useFullscreenViewStore.getState().session).not.toBeNull();
    }
    useFullscreenViewStore.setState({ pendingDelete: { kind: "permanent", key: "b", fileName: "b.jpg" } });
    for (const key of [" ", "Escape", "ArrowRight"]) await handleFullscreenViewKey({ key });
    expect(useFullscreenViewStore.getState().session?.item.hash).toBe("b");
  });

  it("ignores F, which no longer changes any view", async () => {
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    await handleFullscreenViewKey({ key: "f" });
    expect(useFullscreenViewStore.getState().session?.item.hash).toBe("b");
  });

  it("deletes the member the review named even after a refresh advances the sequence", async () => {
    mockCommand("delete_items", () => ({ error: null, failedFiles: 0 }));
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useFullscreenViewStore.getState().session?.item.hash).toBe("b");

    await requestFullscreenViewDelete(true);
    // The watcher reports b removed externally; reconciliation advances the
    // open sequence to c while the permanent-delete review is still open.
    sequenceIndex = 2;
    useFullscreenViewStore.getState().update(sequenceSnapshot() as ViewerSequenceSnapshot);
    expect(useFullscreenViewStore.getState().session?.item.hash).toBe("c");
    expect(fullscreenViewBroadcast().pendingDelete).toEqual({ kind: "permanent", fileName: "b.jpg" });

    await confirmFullscreenViewDelete();

    const deletes = invokeCalls.filter((call) => call.command === "delete_items");
    expect(deletes).toHaveLength(1);
    expect(deletes[0]!.args).toEqual({
      items: [{ hash: "b", pathId: null }],
      permanent: true,
    });
  });

  it("trashes only the displayed item, never a hidden Main multi-selection (R5.2 T1)", async () => {
    mockCommand("delete_items", () => ({ error: null, failedFiles: 0 }));
    useItemsStore.setState({
      selectedKeys: new Set(["a", "b", "c"]),
      selectedPositions: new Map([["a", 0], ["b", 1], ["c", 2]]),
    });
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useFullscreenViewStore.getState().session?.item.hash).toBe("b");

    await requestFullscreenViewDelete(false);

    const deletes = invokeCalls.filter((call) => call.command === "delete_items");
    expect(deletes).toHaveLength(1);
    expect(deletes[0]!.args).toEqual({
      items: [{ hash: "b", pathId: null }],
      permanent: false,
    });
  });

  it("permanently deletes only the displayed item, never a hidden Main multi-selection (R5.2 T1)", async () => {
    mockCommand("delete_items", () => ({ error: null, failedFiles: 0 }));
    useItemsStore.setState({
      selectedKeys: new Set(["a", "b", "c"]),
      selectedPositions: new Map([["a", 0], ["b", 1], ["c", 2]]),
    });
    expect(openFullscreenView()).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useFullscreenViewStore.getState().session?.item.hash).toBe("b");

    await requestFullscreenViewDelete(true);
    await confirmFullscreenViewDelete();

    const deletes = invokeCalls.filter((call) => call.command === "delete_items");
    expect(deletes).toHaveLength(1);
    expect(deletes[0]!.args).toEqual({
      items: [{ hash: "b", pathId: null }],
      permanent: true,
    });
  });
});

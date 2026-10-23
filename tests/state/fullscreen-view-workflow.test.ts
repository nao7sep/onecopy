// @vitest-environment happy-dom
//
// The workflows under test record failures through the notifications store,
// which renders the sentence in the language the document declares, so these
// specs need a document even though the subject is pure workflow logic.

import { EMPTY_ITEM_WORK } from "../helpers/itemWork";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SectionItem } from "../../src/models/items";
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

function sequenceSnapshot(token = "fullscreen-view-token", index = sequenceIndex) {
  const current = sequence[index]!;
  return {
    token,
    member: { hash: current.hash, pathId: current.pathId },
    item: current,
    detail: {
      fileName: current.fileName, kind: "image", byteSize: current.byteSize,
      width: current.width, height: current.height, durationMs: null,
      dateState: "dated", resolvedUtcMs: current.resolvedUtcMs,
      resolvedSource: "metadata", dateOnly: false, copyPaths: [], copyCount: 0,
      companionPaths: [], stripFrames: null,
    },
    index,
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
  it("refreshes completed fullscreen snapshots without a Main reconciliation and coalesces bursts", async () => {
    await installFullscreenViewWorkflow();
    openFullscreenView();
    await vi.waitFor(() => expect(useFullscreenViewStore.getState().session).not.toBeNull());
    const original = useFullscreenViewStore.getState().session!;
    useFullscreenViewStore.setState({ session: { ...original, detail: { ...original.detail, kind: "video" } } });
    // Fullscreen remains authoritative even if Main is looking elsewhere.
    useItemsStore.setState({ selectedItem: "unrelated" });
    const requests: Array<(detail: typeof original.detail) => void> = [];
    let active = 0;
    let maximum = 0;
    mockCommand("get_item_detail", () => {
      active += 1;
      maximum = Math.max(maximum, active);
      return new Promise<typeof original.detail>((resolve) => requests.push(resolve))
        .finally(() => { active -= 1; });
    });
    const reconcileBefore = invokeCalls.filter((call) => call.command === "viewer_sequence_reconcile").length;
    const publish = () => fireEvent("derived://item", { previousHash: "b", item: { ...item("b", 2), hasThumb: true } });
    publish();
    await vi.waitFor(() => expect(requests).toHaveLength(1));
    for (let index = 0; index < 500; index += 1) publish();
    expect(requests).toHaveLength(1);
    requests[0]({ ...original.detail, kind: "video", stripFrames: 4 });
    await vi.waitFor(() => expect(requests).toHaveLength(2));
    requests[1]({ ...original.detail, kind: "video", stripFrames: 8 });
    await vi.waitFor(() => expect(fullscreenViewBroadcast().detail?.stripFrames).toBe(8));
    expect(maximum).toBe(1);
    expect(useFullscreenViewStore.getState().session).toMatchObject({
      token: original.token, index: original.index, length: original.length, scope: original.scope,
    });
    expect(invokeCalls.filter((call) => call.command === "viewer_sequence_reconcile")).toHaveLength(reconcileBefore);
    expect(useItemsStore.getState().selectedItem).toBe("unrelated");
  });

  it.each(["navigation", "return", "replacement", "close", "failure", "retry"])(
    "fences a derived detail read through %s",
    async (transition) => {
      await installFullscreenViewWorkflow();
      openFullscreenView();
      await vi.waitFor(() => expect(useFullscreenViewStore.getState().session).not.toBeNull());
      const original = useFullscreenViewStore.getState().session!;
      let resolve!: (detail: typeof original.detail) => void;
      let reject!: (error: Error) => void;
      mockCommand("get_item_detail", ({ hash }) => hash !== "b" || resolve !== undefined
        ? { ...original.detail, fileName: `${String(hash)}.jpg` }
        : new Promise((success, failure) => { resolve = success; reject = failure; }));
      fireEvent("derived://item", { previousHash: "b", item: item("b", 2) });
      await vi.waitFor(() => expect(resolve).toBeDefined());
      if (transition === "navigation" || transition === "return") {
        moveFullscreenView("next");
        await vi.waitFor(() => expect(useFullscreenViewStore.getState().currentKey()).toBe("c"));
        if (transition === "return") {
          moveFullscreenView("previous");
          await vi.waitFor(() => expect(useFullscreenViewStore.getState().currentKey()).toBe("b"));
        }
      } else if (transition === "replacement") {
        openFullscreenView();
        await vi.waitFor(() => expect(invokeCalls.filter((call) => call.command === "viewer_sequence_start")).toHaveLength(2));
      } else if (transition === "close") await closeFullscreenView({ restoreFocus: false });
      if (transition === "failure" || transition === "retry") reject(new Error("index unavailable"));
      else resolve({ ...original.detail, stripFrames: 99 });
      await new Promise((done) => setTimeout(done, 0));
      if (transition === "close") expect(fullscreenViewBroadcast().detail).toBeNull();
      else if (transition === "navigation") {
        expect(fullscreenViewBroadcast().item?.hash).toBe("c");
        expect(fullscreenViewBroadcast().detail?.stripFrames).not.toBe(99);
      } else if (transition === "return" || transition === "replacement") {
        expect(fullscreenViewBroadcast().item?.hash).toBe("b");
        expect(fullscreenViewBroadcast().detail?.stripFrames).not.toBe(99);
      } else expect(useFullscreenViewStore.getState().failure?.key).toBe("fullscreenView.refreshFailed");
      if (transition === "retry") {
        mockCommand("get_item_detail", () => ({ ...original.detail, stripFrames: 8 }));
        fireEvent("derived://item", { previousHash: "b", item: item("b", 2) });
        await vi.waitFor(() => expect(fullscreenViewBroadcast().detail?.stripFrames).toBe(8));
        expect(useFullscreenViewStore.getState().failure).toBeNull();
      }
    },
  );

  it.each(["direction", "jump", "failure", "close"])(
    "bounds repeat debt while a native move is held and honors the latest %s intent",
    async (intent) => {
      openFullscreenView();
      await vi.waitFor(() => expect(useFullscreenViewStore.getState().session).not.toBeNull());
      const movements: unknown[] = [];
      let settle!: () => void;
      mockCommand("viewer_sequence_move", ({ movement }) => {
        movements.push(movement);
        if (movement === "next") sequenceIndex = Math.min(sequence.length - 1, sequenceIndex + 1);
        if (movement === "previous") sequenceIndex = Math.max(0, sequenceIndex - 1);
        if (movement === "first") sequenceIndex = 0;
        if (movement === "last") sequenceIndex = sequence.length - 1;
        if (movements.length === 1) return new Promise((resolve, reject) => {
          settle = () => intent === "failure" ? reject(new Error("read interrupted")) : resolve(sequenceSnapshot());
        });
        return sequenceSnapshot();
      });
      moveFullscreenView("next");
      await vi.waitFor(() => expect(movements).toEqual(["next"]));
      for (let index = 0; index < 500; index += 1) moveFullscreenView("next");
      if (intent === "jump") moveFullscreenView("first");
      moveFullscreenView("previous");
      if (intent === "close") await closeFullscreenView({ restoreFocus: false });
      expect(movements).toEqual(["next"]);
      settle();
      // Let every queued continuation drain, including a regressed repeat FIFO.
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(movements).toEqual(intent === "close" ? ["next"] : intent === "jump"
        ? ["next", "first", "previous"] : ["next", "previous"]);
      if (intent === "close") {
        expect(useFullscreenViewStore.getState().session).toBeNull();
        expect(invokeCalls.some((call) => call.command === "viewer_sequence_close")).toBe(true);
      } else {
        expect(useFullscreenViewStore.getState().currentKey()).toBe(intent === "jump" ? "a" : "b");
        expect(useFullscreenViewStore.getState().failure).toBeNull();
      }
    },
  );

  it("drops pending navigation when a replacement session is admitted", async () => {
    openFullscreenView();
    await vi.waitFor(() => expect(useFullscreenViewStore.getState().session).not.toBeNull());
    let release!: () => void;
    let reads = 0;
    mockCommand("viewer_sequence_move", () => {
      if (++reads > 1) return sequenceSnapshot("fullscreen-view-token", 0);
      return new Promise((resolve) => {
        release = () => resolve(sequenceSnapshot("fullscreen-view-token", 2));
      });
    });
    moveFullscreenView("next");
    await vi.waitFor(() => expect(release).toBeDefined());
    moveFullscreenView("previous");
    openFullscreenView();
    release();
    await vi.waitFor(() => expect(invokeCalls.filter((call) => call.command === "viewer_sequence_start")).toHaveLength(2));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(invokeCalls.filter((call) => call.command === "viewer_sequence_move")).toHaveLength(1);
    expect(useFullscreenViewStore.getState().currentKey()).toBe("b");
    mockCommand("viewer_sequence_move", () => sequenceSnapshot("fullscreen-view-token", 0));
    moveFullscreenView("previous");
    await vi.waitFor(() => expect(useFullscreenViewStore.getState().currentKey()).toBe("a"));
  });

  it("coalesces repeats through the Main anchor lookup as well as native movement", async () => {
    openFullscreenView();
    await vi.waitFor(() => expect(useFullscreenViewStore.getState().session).not.toBeNull());
    await useItemsStore.getState().select({ kind: "other", month: "undated" });
    let release!: () => void;
    mockCommand("get_item_section", () => new Promise((resolve) => {
      release = () => resolve({ kind: "image", month: "2026-01" });
    }));
    moveFullscreenView("next");
    await vi.waitFor(() => expect(release).toBeDefined());
    for (let index = 0; index < 500; index += 1) moveFullscreenView("next");
    moveFullscreenView("previous");
    expect(invokeCalls.filter((call) => call.command === "viewer_sequence_move")).toHaveLength(1);
    release();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(invokeCalls.filter((call) => call.command === "viewer_sequence_move")
      .map((call) => call.args.movement)).toEqual(["next", "previous"]);
    expect(useItemsStore.getState().selectedItem).toBe("b");
  });

  it("keeps the latest open navigable when an earlier native start settles late", async () => {
    sequence = [item("a", 1), item("b", 2), item("c", 3)];
    let active: ViewerSequenceSnapshot | null = null;
    const starts = new Map<string, () => void>();
    mockCommand("viewer_sequence_start", ({ anchor }) => {
      const hash = (anchor as { hash: string }).hash;
      return new Promise<ViewerSequenceSnapshot>((resolve) => {
        let settled = false;
        starts.set(hash, () => {
          if (settled) return;
          settled = true;
          // Native start replaces the singleton when its build completes.
          active = sequenceSnapshot(hash, hash === "a" ? 0 : 1) as ViewerSequenceSnapshot;
          resolve(active);
        });
      });
    });
    mockCommand("viewer_sequence_close", ({ token }) => {
      if (active?.token === token) active = null;
      return null;
    });
    mockCommand("viewer_sequence_move", ({ token }) => {
      if (active?.token !== token) throw new Error("viewer sequence is closed");
      active = sequenceSnapshot(String(token), 2) as ViewerSequenceSnapshot;
      return active;
    });
    useItemsStore.getState().selectItem("a", "nearest", 0);
    openFullscreenView();
    await vi.waitFor(() => expect(starts.has("a")).toBe(true));
    useItemsStore.getState().selectItem("b", "nearest", 1);
    openFullscreenView();
    await Promise.resolve();
    // Reproduce completion-order replacement if a second start was admitted
    // before the first settled; serialized starts cannot reach this branch.
    starts.get("b")?.();
    await Promise.resolve();
    starts.get("a")!();
    await vi.waitFor(() => expect(starts.has("b")).toBe(true));
    starts.get("b")!();
    await vi.waitFor(() => expect(useFullscreenViewStore.getState().session?.token).toBe("b"));

    moveFullscreenView("next");
    await vi.waitFor(() => expect(useFullscreenViewStore.getState().session?.item.hash).toBe("c"));
    expect(useFullscreenViewStore.getState().failure).toBeNull();
    expect(active).toMatchObject({ token: "b" });
  });

  it.each(["command", "deactivation"] as const)("cancels pending and queued starts on %s without reopening", async (entry) => {
    if (entry === "deactivation") await installFullscreenViewWorkflow();
    sequence = [item("a", 1), item("b", 2), item("c", 3)];
    let finishStart!: () => void;
    mockCommand("viewer_sequence_start", () => new Promise<ViewerSequenceSnapshot>((resolve) => {
      finishStart = () => resolve(sequenceSnapshot("pending") as ViewerSequenceSnapshot);
    }));
    openFullscreenView();
    await vi.waitFor(() => expect(finishStart).toBeDefined());
    openFullscreenView();
    if (entry === "command") await closeFullscreenView();
    else fireEvent("app://activation", false);
    finishStart();
    await vi.waitFor(() => expect(invokeCalls.some((call) =>
      call.command === "viewer_sequence_close" && call.args.token === "pending",
    )).toBe(true));
    // Settle a wrongly admitted second start too, so the regression's failure
    // cannot leave the shared sequence queue blocked for later tests.
    await new Promise((resolve) => setTimeout(resolve, 0));
    if (invokeCalls.filter((call) => call.command === "viewer_sequence_start").length > 1) finishStart();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(invokeCalls.filter((call) => call.command === "viewer_sequence_start")).toHaveLength(1);
    expect(useFullscreenViewStore.getState().session).toBeNull();
    const window = await WebviewWindow.getByLabel("fullscreen-view");
    expect(window?.show).not.toHaveBeenCalled();
  });

  it.each(["command", "deactivation"] as const)(
    "retires an old window before reopening while %s cleanup is delayed",
    async (entry) => {
      await installFullscreenViewWorkflow();
      const window = await WebviewWindow.getByLabel("fullscreen-view");
      let token = 0;
      mockCommand("viewer_sequence_start", () => {
        sequence = [item("a", 1), item("b", 2), item("c", 3)];
        return sequenceSnapshot(`session-${++token}`, 1);
      });
      openFullscreenView();
      await vi.waitFor(() => expect(window?.show).toHaveBeenCalledOnce());
      let finishCleanup!: () => void;
      mockCommand("viewer_sequence_close", () => new Promise<void>((resolve) => {
        finishCleanup = resolve;
      }));
      setFocus.mockClear();
      const closing = entry === "command" ? closeFullscreenView() : undefined;
      if (entry === "deactivation") fireEvent("app://activation", false);
      await vi.waitFor(() => expect(finishCleanup).toBeDefined());
      setFocus.mockClear();
      openFullscreenView();
      await new Promise((resolve) => setTimeout(resolve, 0));
      finishCleanup();
      await closing;
      await vi.waitFor(() => expect(window?.show).toHaveBeenCalledTimes(2));
      await new Promise((resolve) => setTimeout(resolve, 0));

      expect(useFullscreenViewStore.getState().session?.token).toBe("session-2");
      expect(window?.hide).toHaveBeenCalledOnce();
      expect(window!.hide.mock.invocationCallOrder[0]).toBeLessThan(window!.show.mock.invocationCallOrder[1]!);
      expect(setFocus).not.toHaveBeenCalled();
    },
  );

  it("settles reconciliation that closes an empty native sequence", async () => {
    await installFullscreenViewWorkflow();
    const window = await WebviewWindow.getByLabel("fullscreen-view");
    openFullscreenView();
    await vi.waitFor(() => expect(window?.show).toHaveBeenCalledOnce());
    mockCommand("viewer_sequence_reconcile", () => null);
    useItemsStore.setState({ reconciliationId: useItemsStore.getState().reconciliationId + 1 });
    await vi.waitFor(() => expect(window?.hide).toHaveBeenCalledOnce());
    await vi.waitFor(() => expect(invokeCalls.some((call) => call.command === "viewer_sequence_close")).toBe(true));
    expect(useFullscreenViewStore.getState().session).toBeNull();
    openFullscreenView();
    await vi.waitFor(() => expect(window?.show).toHaveBeenCalledTimes(2));
  });

  it("does not restore Main focus after a newer open is admitted during window retirement", async () => {
    const window = (await WebviewWindow.getByLabel("fullscreen-view"))!;
    openFullscreenView();
    await vi.waitFor(() => expect(window.show).toHaveBeenCalledOnce());
    let finishHide!: () => void;
    window.hide.mockImplementationOnce(() => new Promise<void>((resolve) => { finishHide = resolve; }));
    const closing = closeFullscreenView();
    await vi.waitFor(() => expect(finishHide).toBeDefined());
    setFocus.mockClear();
    openFullscreenView();
    finishHide();
    await closing;
    await vi.waitFor(() => expect(window.show).toHaveBeenCalledTimes(2));
    expect(setFocus).not.toHaveBeenCalled();
  });

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

  it("lets an admitted Main restore finish when the fullscreen view closes", async () => {
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

  it("keeps navigation failure on the fullscreen view while recording only Recent history", async () => {
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
    expect(fullscreenViewBroadcast().pendingDelete).toEqual({ kind: "permanent", key: "b", fileName: "b.jpg" });

    await confirmFullscreenViewDelete();

    const deletes = invokeCalls.filter((call) => call.command === "delete_items");
    expect(deletes).toHaveLength(1);
    expect(deletes[0]!.args).toEqual({
      items: [{ hash: "b", pathId: null }],
      permanent: true,
    });
  });

  it("trashes only the displayed item, never a hidden Main multi-selection", async () => {
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

  it("permanently deletes only the displayed item, never a hidden Main multi-selection", async () => {
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

it("refuses stale fullscreen deletion keys and accepts the displayed session member", async () => {
  mockCommand("delete_items", () => ({ error: null, failedFiles: 0 }));
  expect(openFullscreenView()).toBe(true);
  await vi.waitFor(() => expect(useFullscreenViewStore.getState().session?.item.hash).toBe("b"));
  const shown = fullscreenViewBroadcast();
  sequenceIndex = 2;
  useFullscreenViewStore.getState().update(sequenceSnapshot() as ViewerSequenceSnapshot);
  await handleFullscreenViewKey({ key: "Delete", token: shown.token, itemKey: "b" });
  await handleFullscreenViewKey({ key: "Delete", token: "old-session", itemKey: "c" });
  await handleFullscreenViewKey({ key: "Delete" });
  expect(invokeCalls.filter((call) => call.command === "delete_items")).toEqual([]);
  expect(useFullscreenViewStore.getState().pendingDelete).toBeNull();
  await handleFullscreenViewKey({ key: "Delete", token: shown.token, itemKey: "c" });
  expect(invokeCalls.filter((call) => call.command === "delete_items").at(-1)?.args)
    .toEqual({ items: [{ hash: "c", pathId: null }], permanent: false });
});

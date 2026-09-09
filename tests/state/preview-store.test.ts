// Preview owns one current identity/detail package. Cross-window delivery
// coalesces rapid changes without losing matching details or reviving closed
// and cleared selections.

import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { usePreviewStore } from "../../src/state/preview-store";
import {
  emitCalls,
  invokeCalls,
  mockCommands,
  resetTauriMocks,
  rejectNextWindowListener,
  WebviewWindow,
} from "../mocks/tauri";

const ITEM_A = { hash: "ha", pathId: null };
const ITEM_B = { hash: "hb", pathId: null };

function detailFor(name: string) {
  return {
    fileName: name,
    kind: "image",
    byteSize: 1,
    width: 1,
    height: 1,
    durationMs: null,
    dateState: "dated" as const,
    resolvedUtcMs: 0,
    resolvedSource: "metadata",
    dateOnly: false,
    copyPaths: [],
    companionPaths: [],
    stripFrames: null,
  };
}

/** Follow armed in the in-window split placement, so no real window is made. */
function armSplitFollow(): void {
  usePreviewStore.setState({
    follow: true,
    placement: "split",
    current: null,
  });
}

// Fake time exercises publications on both sides of the throttle boundary.
beforeAll(() => vi.useFakeTimers());
afterAll(() => vi.useRealTimers());

beforeEach(() => {
  usePreviewStore.getState().close();
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    patch_state: () => ({}),
    get_item_detail: () => null,
    log_event: () => null,
    record_recent_notification: () => ({}),
    set_window_fullscreen: () => null,
  });
  usePreviewStore.setState({
    follow: false,
    placement: null,
    placementPreference: null,
    current: null,
    fullscreen: false,
    error: null,
  });
});

describe("the stale-detail guard", () => {
  it("retains B detail arriving before the trailing window publication", async () => {
    new WebviewWindow("preview");
    usePreviewStore.setState({ placementPreference: "window" });
    await usePreviewStore.getState().open(ITEM_A, detailFor("A.jpg"));
    usePreviewStore.getState().anchorChanged(ITEM_A, null);
    usePreviewStore.getState().anchorChanged(ITEM_B, null);
    usePreviewStore.getState().detailLoaded(ITEM_B, detailFor("B.jpg"));
    usePreviewStore.getState().detailLoaded(ITEM_A, detailFor("late A.jpg"));

    await vi.advanceTimersByTimeAsync(200);

    expect(usePreviewStore.getState().current).toEqual({
      ...ITEM_B, detail: detailFor("B.jpg"),
    });
    expect(emitCalls.filter((call) => call.event === "preview://show").at(-1)?.payload)
      .toEqual({ ...ITEM_B, detail: detailFor("B.jpg") });
  });

  it("drops a late detail for an anchor the user already left", () => {
    armSplitFollow();
    usePreviewStore.getState().anchorChanged(ITEM_A, null);
    usePreviewStore.getState().anchorChanged(ITEM_B, null);
    expect(usePreviewStore.getState().current?.hash).toBe("hb");

    // A's detail arrives after the anchor moved to B.
    usePreviewStore.getState().detailLoaded(ITEM_A, detailFor("A.jpg"));

    expect(usePreviewStore.getState().current?.hash).toBe("hb");
    expect(usePreviewStore.getState().current?.detail).toBeFalsy();
  });

  it("completes the message when the detail is for the current anchor", () => {
    armSplitFollow();
    usePreviewStore.getState().anchorChanged(ITEM_A, null);

    usePreviewStore.getState().detailLoaded(ITEM_A, detailFor("A.jpg"));

    expect(usePreviewStore.getState().current?.detail?.fileName).toBe("A.jpg");
  });

  it("ignores a detail entirely while follow is off", () => {
    usePreviewStore.setState({ follow: false, placement: null, current: null });
    usePreviewStore.getState().detailLoaded(ITEM_A, detailFor("A.jpg"));
    expect(usePreviewStore.getState().current).toBeNull();
  });
});

describe("the follow throttle", () => {
  it("keeps local identity current while coalescing cross-window publication", async () => {
    new WebviewWindow("preview");
    usePreviewStore.setState({ placementPreference: "window" });
    await usePreviewStore.getState().open(ITEM_A, null);
    emitCalls.length = 0;
    vi.advanceTimersByTime(200);
    const anchors = ["h1", "h2", "h3", "h4", "h5"];
    for (const hash of anchors) {
      usePreviewStore.getState().anchorChanged({ hash, pathId: null }, null);
    }
    expect(usePreviewStore.getState().current?.hash).toBe("h5");
    expect(emitCalls.filter((call) => call.event === "preview://show"))
      .toEqual([{ event: "preview://show", payload: { hash: "h1", pathId: null, detail: null } }]);

    vi.advanceTimersByTime(200);
    expect(emitCalls.filter((call) => call.event === "preview://show").at(-1)?.payload)
      .toEqual({ hash: "h5", pathId: null, detail: null });
  });
});

describe("clearing the surface", () => {
  it.each(["close", "anchorCleared"] as const)("invalidates queued delivery on %s", async (action) => {
    new WebviewWindow("preview");
    usePreviewStore.setState({ placementPreference: "window" });
    await usePreviewStore.getState().open(ITEM_A, null);
    usePreviewStore.getState().anchorChanged(ITEM_A, null);
    usePreviewStore.getState().anchorChanged(ITEM_B, null);
    usePreviewStore.getState()[action]();
    const publishedCount = emitCalls.length;

    await vi.advanceTimersByTimeAsync(200);

    expect(emitCalls).toHaveLength(publishedCount);
    expect(usePreviewStore.getState().current).toEqual(
      action === "close" ? null : { hash: null, pathId: null, detail: null },
    );
  });

  it("stops showing an item once follow is turned off", () => {
    armSplitFollow();
    usePreviewStore.getState().anchorChanged(ITEM_A, null);
    expect(usePreviewStore.getState().current?.hash).toBe("ha");

    usePreviewStore.getState().close();

    expect(usePreviewStore.getState().current).toBeNull();
    expect(usePreviewStore.getState().follow).toBe(false);
  });

  it("emits nothing to a window that was never opened", () => {
    armSplitFollow();
    usePreviewStore.getState().anchorChanged(ITEM_A, null);
    // The split placement renders in-window; no preview://show is warranted.
    expect(emitCalls.filter((c) => c.event === "preview://show")).toHaveLength(0);
  });
});

describe("preview window failures", () => {
  it("round-trips fullscreen on the live Preview and keeps focus there", async () => {
    const window = new WebviewWindow("preview");
    usePreviewStore.setState({ placementPreference: "window" });
    await usePreviewStore.getState().open(ITEM_A, detailFor("A.jpg"));

    await usePreviewStore.getState().setFullscreen(true);
    usePreviewStore.getState().anchorChanged(ITEM_B, detailFor("B.jpg"));
    await vi.advanceTimersByTimeAsync(200);
    await usePreviewStore.getState().setFullscreen(false);
    await usePreviewStore.getState().setFullscreen(true);

    expect(usePreviewStore.getState().current?.detail?.fileName).toBe("B.jpg");
    expect(usePreviewStore.getState().fullscreen).toBe(true);
    expect(window.setFocus).toHaveBeenCalledTimes(3);
    expect(invokeCalls.filter((call) => call.command === "set_window_fullscreen").slice(-3).map((call) => call.args))
      .toEqual([
        { label: "preview", enable: true }, { label: "preview", enable: false },
        { label: "preview", enable: true },
      ]);
    expect(invokeCalls.some((call) => call.command === "viewer_sequence_start")).toBe(false);
  });

  it("does not lose queued detail when the placement is unchanged", async () => {
    new WebviewWindow("preview");
    usePreviewStore.setState({ placementPreference: "window" });
    await usePreviewStore.getState().open(ITEM_A, null);
    usePreviewStore.getState().detailLoaded(ITEM_A, detailFor("A.jpg"));

    await usePreviewStore.getState().setPlacementPreference("window");
    await vi.advanceTimersByTimeAsync(200);

    expect(emitCalls.filter((call) => call.event === "preview://show").at(-1)?.payload)
      .toEqual({ ...ITEM_A, detail: detailFor("A.jpg") });
  });

  it.each(["open", "placement"] as const)("publishes the latest package after delayed %s", async (entry) => {
    const window = new WebviewWindow("preview");
    let finishShow: (() => void) | undefined;
    window.show.mockImplementationOnce(() => new Promise<void>((resolve) => {
      finishShow = resolve;
    }));
    armSplitFollow();
    usePreviewStore.getState().anchorChanged(ITEM_A, detailFor("A.jpg"));
    if (entry === "open") usePreviewStore.setState({ placementPreference: "window" });
    const opening = entry === "open"
      ? usePreviewStore.getState().open(ITEM_A, detailFor("A.jpg"))
      : usePreviewStore.getState().setPlacementPreference("window");
    for (let index = 0; index < 10 && !finishShow; index += 1) await Promise.resolve();
    expect(finishShow).toBeDefined();

    usePreviewStore.getState().anchorChanged(ITEM_B, null);
    usePreviewStore.getState().detailLoaded(ITEM_B, detailFor("B.jpg"));
    finishShow?.();
    await opening;
    await vi.advanceTimersByTimeAsync(200);

    expect(emitCalls.filter((call) => call.event === "preview://show").at(-1)?.payload)
      .toEqual({ ...ITEM_B, detail: detailFor("B.jpg") });
  });

  it("cancels window publication when moving the latest package to the pane", async () => {
    new WebviewWindow("preview");
    usePreviewStore.setState({ placementPreference: "window" });
    await usePreviewStore.getState().open(ITEM_A, detailFor("A.jpg"));
    usePreviewStore.getState().anchorChanged(ITEM_B, null);
    usePreviewStore.getState().detailLoaded(ITEM_B, detailFor("B.jpg"));

    await usePreviewStore.getState().setPlacementPreference("split");
    const publishedCount = emitCalls.length;
    await vi.advanceTimersByTimeAsync(200);

    expect(emitCalls).toHaveLength(publishedCount);
    expect(usePreviewStore.getState()).toMatchObject({
      placement: "split", current: { ...ITEM_B, detail: detailFor("B.jpg") },
    });
  });

  it("settles a rejected creation-listener registration as an authored Preview failure", async () => {
    rejectNextWindowListener(
      new Error("TypeError: EACCES /private/tmp/preview listener sentinel"),
    );
    usePreviewStore.setState({ placementPreference: "window" });

    await usePreviewStore.getState().open(ITEM_A, detailFor("A.jpg"));

    expect(usePreviewStore.getState().error).toBe(
      "Couldn’t open the Preview window.",
    );
    expect(usePreviewStore.getState().error).not.toContain("EACCES");
    expect(
      invokeCalls.some((call) => call.command === "record_recent_notification"),
    ).toBe(true);
  });

  it("serializes a window-to-split-to-window change so an older close cannot win", async () => {
    const window = new WebviewWindow("preview");
    let finishClose: (() => void) | undefined;
    window.destroy.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          finishClose = resolve;
        }),
    );
    usePreviewStore.setState({
      follow: true,
      placement: "window",
      placementPreference: "window",
      current: { ...ITEM_A, detail: detailFor("A.jpg") },
    });

    const split = usePreviewStore.getState().setPlacementPreference("split");
    for (let index = 0; index < 10 && !finishClose; index += 1) {
      await Promise.resolve();
    }
    expect(window.destroy).toHaveBeenCalledOnce();
    const backToWindow = usePreviewStore
      .getState()
      .setPlacementPreference("window");
    expect(window.show).not.toHaveBeenCalled();

    finishClose?.();
    await Promise.all([split, backToWindow]);

    expect(usePreviewStore.getState()).toMatchObject({
      follow: true,
      placement: "window",
      placementPreference: "window",
    });
    expect(window.show).toHaveBeenCalledOnce();
  });

  it("keeps the failure on Preview while recording only Recent history", async () => {
    const window = new WebviewWindow("preview");
    window.show.mockRejectedValueOnce(new Error("window unavailable"));
    usePreviewStore.setState({ placementPreference: "window" });

    await usePreviewStore.getState().open(ITEM_A, detailFor("A.jpg"));

    expect(usePreviewStore.getState().error).toBe(
      "Couldn’t open the Preview window.",
    );
    expect(
      invokeCalls.some((call) => call.command === "record_recent_notification"),
    ).toBe(true);
    expect(
      invokeCalls.some((call) => call.command === "publish_notification"),
    ).toBe(false);
  });
});

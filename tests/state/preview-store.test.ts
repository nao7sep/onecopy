// The preview-follow stream.
//
// The module header claims the stale-detail race is designed out and that the
// follow path is throttled; nothing proved either. Both matter at the
// keyboard: a slow detail painting a superseded anchor shows the wrong file
// name beside the right image, and an unthrottled stream emits once per
// arrow key while a held key repeats.

import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { usePreviewStore } from "../../src/state/preview-store";
import {
  emitCalls,
  invokeCalls,
  mockCommands,
  resetTauriMocks,
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

// The follow throttle keeps its clock in module scope, so without fake timers
// one spec's delivery leaves the window closed for the next and every later
// anchorChanged lands on the trailing path instead of the leading one.
beforeAll(() => vi.useFakeTimers());
afterAll(() => vi.useRealTimers());

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    patch_state: () => ({}),
    get_item_detail: () => null,
    log_event: () => null,
    record_recent_notification: () => ({}),
  });
  usePreviewStore.setState({
    follow: false,
    placement: null,
    placementPreference: null,
    current: null,
    error: null,
  });
  // Step past the throttle window so each spec starts on a leading edge.
  vi.advanceTimersByTime(500);
});

describe("the stale-detail guard", () => {
  it("drops a late detail for an anchor the user already left", () => {
    armSplitFollow();
    usePreviewStore.getState().anchorChanged(ITEM_A, null);
    // Past the throttle window, so B is actually delivered rather than left
    // pending behind A's leading edge.
    vi.advanceTimersByTime(200);
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
  it("emits the leading anchor immediately and coalesces the rest", () => {
    {
      armSplitFollow();
      const anchors = ["h1", "h2", "h3", "h4", "h5"];
      for (const hash of anchors) {
        usePreviewStore.getState().anchorChanged({ hash, pathId: null }, null);
      }
      // Leading edge only so far — the rest are pending.
      expect(usePreviewStore.getState().current?.hash).toBe("h1");

      vi.advanceTimersByTime(200);
      // The trailing edge carries the LAST anchor, not the second one: a held
      // arrow key must land on where the user stopped.
      expect(usePreviewStore.getState().current?.hash).toBe("h5");
    }
  });
});

describe("clearing the surface", () => {
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

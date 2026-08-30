import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  comparisonKeyIsRoutable,
  handleComparisonKey,
} from "../../src/workflows/comparison";
import {
  useComparisonStore,
  type GroupMember,
} from "../../src/state/comparison-store";

function member(index: number): GroupMember {
  return {
    hash: `h${index}`,
    fileName: `image-${index}.jpg`,
    width: 4000,
    height: 3000,
    byteSize: 1000,
    sharpness: null,
    faceScore: null,
    copyCount: 1,
    hasThumb: true,
  };
}

function openSession(count = 4): void {
  const members = Array.from({ length: count }, (_, index) => member(index));
  useComparisonStore.setState({
    sessionId: 0,
    open: true,
    members,
    originalMemberHashes: members.map((item) => item.hash),
    page: 0,
    maximumImages: 16,
    displayCount: 1,
    displayAspects: [16 / 9],
    capacities: [4],
    portraitDominant: false,
    spreadCount: 0,
    selected: new Set(["h0"]),
    anchors: new Set(["h0"]),
    anchor: "h0",
    rangeOrigin: "h0",
    rangeBase: new Set(["h0"]),
    busy: false,
    message: null,
    pendingAction: null,
    failure: null,
  });
}

beforeEach(() => {
  openSession();
});

describe("comparison keyboard selection", () => {
  it("uses a direct key as a neutral toggle", () => {
    expect(handleComparisonKey({ key: "1" })).toBe(true);
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h0", "h1"]),
    );
    expect(useComparisonStore.getState().anchor).toBe("h1");
  });

  it("moves spatially and Shift extends from the range origin", () => {
    handleComparisonKey({ key: "ArrowRight" });
    expect(useComparisonStore.getState().selected).toEqual(new Set(["h2"]));
    handleComparisonKey({ key: "ArrowDown", shiftKey: true });
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h2", "h3"]),
    );
  });

  it("preserves deliberate toggles outside a Shift range and lets the range shrink", () => {
    handleComparisonKey({ key: "3" });
    handleComparisonKey({ key: "ArrowUp", shiftKey: true });
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h0", "h2", "h3"]),
    );
    handleComparisonKey({ key: "ArrowDown", shiftKey: true });
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h0", "h3"]),
    );
  });

  it("selects only the current page with Cmd/Ctrl+A", () => {
    expect(handleComparisonKey({ key: "a", metaKey: true })).toBe(true);
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h0", "h1", "h2", "h3"]),
    );
  });

  it("keeps Space as an intentional no-op", () => {
    const before = useComparisonStore.getState().selected;
    expect(handleComparisonKey({ key: " " })).toBe(true);
    expect(useComparisonStore.getState().selected).toBe(before);
  });

  it("does not repeat direct toggles", () => {
    expect(handleComparisonKey({ key: "1", repeat: true })).toBe(false);
    expect(useComparisonStore.getState().selected).toEqual(new Set(["h0"]));
  });

  it("leaves modified and unassigned keys to the app or operating system", () => {
    expect(handleComparisonKey({ key: "ArrowRight", metaKey: true })).toBe(
      false,
    );
    expect(handleComparisonKey({ key: "f" })).toBe(false);
    expect(comparisonKeyIsRoutable({ key: "a" }, 1)).toBe(false);
    expect(useComparisonStore.getState().selected).toEqual(new Set(["h0"]));
  });
});

describe("comparison page keys", () => {
  it("uses Page Up and Page Down for paging, not arrow keys", () => {
    openSession(9);
    useComparisonStore.setState({ maximumImages: 4 });
    expect(handleComparisonKey({ key: "PageDown" })).toBe(true);
    expect(useComparisonStore.getState().page).toBe(1);
    expect(handleComparisonKey({ key: "PageUp" })).toBe(true);
    expect(useComparisonStore.getState().page).toBe(0);
  });

  it("routes Escape through close", async () => {
    const close = vi.spyOn(useComparisonStore.getState(), "close");
    expect(handleComparisonKey({ key: "Escape" })).toBe(true);
    expect(close).toHaveBeenCalledOnce();
  });
});

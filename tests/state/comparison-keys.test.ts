// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  comparisonKeyIsRoutable,
  handleComparisonKey,
} from "../../src/workflows/comparison";
import {
  useComparisonStore,
  type GroupMember,
} from "../../src/state/comparison-store";
import { openComparisonImage } from "../../src/workflows/comparison-image";

vi.mock("../../src/workflows/comparison-image", async (importOriginal) => ({
  ...await importOriginal<typeof import("../../src/workflows/comparison-image")>(),
  openComparisonImage: vi.fn(async () => {}),
}));

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
    selected: new Set(),
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
  it("uses a direct key as an explicit keep-mark toggle", () => {
    expect(handleComparisonKey({ key: "1" })).toBe(true);
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h1"]),
    );
    expect(useComparisonStore.getState().anchor).toBe("h1");
  });

  it("moves spatially and Shift extends from the range origin", () => {
    handleComparisonKey({ key: "ArrowRight" });
    expect(useComparisonStore.getState().selected).toEqual(new Set());
    expect(useComparisonStore.getState().anchor).toBe("h1");
    handleComparisonKey({ key: "ArrowDown", shiftKey: true });
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h1", "h2", "h3"]),
    );
  });

  it("preserves deliberate toggles outside a Shift range and lets the range shrink", () => {
    handleComparisonKey({ key: "3" });
    handleComparisonKey({ key: "ArrowUp", shiftKey: true });
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h1", "h2", "h3"]),
    );
    handleComparisonKey({ key: "ArrowDown", shiftKey: true });
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h3"]),
    );
  });

  it("selects only the current page with Cmd/Ctrl+A", () => {
    expect(handleComparisonKey({ key: "a", metaKey: true })).toBe(true);
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h0", "h1", "h2", "h3"]),
    );
  });

  it("uses Space for the image window without toggling a keep mark", () => {
    expect(handleComparisonKey({ key: " " })).toBe(true);
    expect(openComparisonImage).toHaveBeenCalled();
    expect(useComparisonStore.getState().selected).toEqual(new Set());
  });

  it("starts Arrow navigation from the first card without inventing keep intent", () => {
    useComparisonStore.setState({ anchor: null, anchors: new Set(), rangeOrigin: null });
    handleComparisonKey({ key: "ArrowRight" });
    expect(useComparisonStore.getState().anchor).toBe("h0");
    expect(useComparisonStore.getState().selected.size).toBe(0);
  });

  it("does not repeat direct toggles", () => {
    expect(handleComparisonKey({ key: "1", repeat: true })).toBe(false);
    expect(handleComparisonKey({ key: " ", repeat: true })).toBe(true);
    expect(useComparisonStore.getState().selected).toEqual(new Set());
  });

  it("leaves modified and unassigned keys to the app or operating system", () => {
    expect(handleComparisonKey({ key: "ArrowRight", metaKey: true })).toBe(
      false,
    );
    expect(handleComparisonKey({ key: "f" })).toBe(false);
    expect(comparisonKeyIsRoutable({ key: "a" }, 1)).toBe(false);
    expect(useComparisonStore.getState().selected).toEqual(new Set());
  });

  it("consumes repeated destructive keys without changing the page", () => {
    expect(handleComparisonKey({ key: "Enter", repeat: true })).toBe(true);
    expect(handleComparisonKey({ key: "Delete", repeat: true })).toBe(true);
    expect(useComparisonStore.getState()).toMatchObject({
      open: true,
      pendingAction: null,
    });
  });
});

describe("comparison page keys", () => {
  it("uses unmodified Enter with no keep marks to close", async () => {
    const close = vi.spyOn(useComparisonStore.getState(), "close");
    expect(handleComparisonKey({ key: "Enter" })).toBe(true);
    await vi.waitFor(() => expect(close).toHaveBeenCalledOnce());
    close.mockRestore();
  });

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

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
import { createdWindows, invokeCalls, mockCommand } from "../mocks/tauri";

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

  it("does not invent an active card when Cmd/Ctrl+A marks a page with none active", () => {
    useComparisonStore.setState({ anchor: null, anchors: new Set(), rangeOrigin: null });
    expect(handleComparisonKey({ key: "a", metaKey: true })).toBe(true);
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h0", "h1", "h2", "h3"]),
    );
    expect(useComparisonStore.getState().anchor).toBeNull();
  });

  // Nothing enlarges an image from Comparison: Space is taken so no focused
  // control acts on it, and it changes neither the marks nor any window.
  it("takes Space and does nothing with it", () => {
    const before = useComparisonStore.getState();
    const windows = createdWindows.length;
    const commands = invokeCalls.length;
    expect(comparisonKeyIsRoutable({ key: " " })).toBe(true);
    expect(handleComparisonKey({ key: " " })).toBe(true);
    expect(useComparisonStore.getState().selected).toEqual(new Set());
    expect(useComparisonStore.getState().anchor).toBe(before.anchor);
    expect(createdWindows).toHaveLength(windows);
    expect(invokeCalls).toHaveLength(commands);
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

  it.each(["Delete", "Backspace"])(
    "routes %s to the same single-mark direct trash (G1)",
    async (key) => {
      mockCommand("delete_items", ({ items }) => ({
        error: null, failedFiles: 0,
        items: (items as Array<{ hash: string | null; pathId: number | null }>)
          .map((item) => ({ item, failedFiles: 0 })),
      }));
      useComparisonStore.getState().selectSlot(1, "toggle");
      expect(handleComparisonKey({ key })).toBe(true);
      await vi.waitFor(() =>
        expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(true),
      );
      const deleted = invokeCalls.find((call) => call.command === "delete_items");
      expect(deleted?.args.items).toEqual([{ hash: "h1", pathId: null }]);
      expect(deleted?.args.permanent).toBe(false);
      // This file's invokeCalls accumulates across its own tests (unlike
      // files that call resetTauriMocks per test); clear it so a later
      // negative "delete_items was never called" assertion is not fooled by
      // this test's own successful delete.
      invokeCalls.length = 0;
    },
  );

  it("routes Shift+Delete to a permanent review, never a direct trash (G2)", async () => {
    useComparisonStore.getState().selectSlot(1, "toggle");
    expect(handleComparisonKey({ key: "Delete", shiftKey: true })).toBe(true);
    await vi.waitFor(() =>
      expect(useComparisonStore.getState().pendingAction).not.toBeNull(),
    );
    expect(useComparisonStore.getState().pendingAction).toMatchObject({
      permanent: true,
      targetHashes: ["h1"],
    });
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(false);
  });
});

describe("comparison page keys", () => {
  it("uses unmodified Enter with no keep marks to close", async () => {
    const close = vi.spyOn(useComparisonStore.getState(), "close");
    expect(handleComparisonKey({ key: "Enter" })).toBe(true);
    await vi.waitFor(() => expect(close).toHaveBeenCalledOnce());
    close.mockRestore();
  });

  it("also uses Shift+Enter with no keep marks to close, not a permanent decision", async () => {
    const close = vi.spyOn(useComparisonStore.getState(), "close");
    expect(handleComparisonKey({ key: "Enter", shiftKey: true })).toBe(true);
    await vi.waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
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

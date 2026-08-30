import { beforeEach, describe, expect, it } from "vitest";
import {
  useComparisonStore,
  type GroupMember,
} from "../../src/state/comparison-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

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

function openSession(count: number): void {
  const members = Array.from({ length: count }, (_, index) => member(index));
  useComparisonStore.setState({
    sessionId: 0,
    open: true,
    members,
    originalMemberHashes: members.map((item) => item.hash),
    page: 0,
    maximumImages: 4,
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

function successfulDelete(
  items: Array<{ hash: string | null; pathId: number | null }>,
) {
  return {
    cancelled: false,
    error: null,
    failedFiles: 0,
    items: items.map((item) => ({ item, failedFiles: 0 })),
  };
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    delete_items: ({ items }) =>
      successfulDelete(
        items as Array<{ hash: string | null; pathId: number | null }>,
      ),
    set_window_simple_fullscreen: () => null,
  });
  openSession(8);
});

describe("draft page selection", () => {
  it("retains each undecided page draft while browsing", () => {
    useComparisonStore.getState().selectSlot(1, "toggle");
    useComparisonStore.getState().nextPage();
    useComparisonStore.getState().selectSlot(2, "exclusive");
    useComparisonStore.getState().prevPage();

    const state = useComparisonStore.getState();
    expect(state.selected).toEqual(new Set(["h0", "h1", "h6"]));
    expect(state.anchor).toBe("h1");
  });

  it("preserves a deliberately empty page draft", () => {
    useComparisonStore.getState().selectSlot(0, "toggle");
    expect(useComparisonStore.getState().selected).toEqual(new Set());

    useComparisonStore.getState().nextPage();
    useComparisonStore.getState().prevPage();

    const state = useComparisonStore.getState();
    expect(state.selected).toEqual(new Set());
    expect(state.anchor).toBeNull();
  });

  it("does not wrap at either page bound", () => {
    useComparisonStore.getState().prevPage();
    expect(useComparisonStore.getState().page).toBe(0);
    useComparisonStore.getState().nextPage();
    useComparisonStore.getState().nextPage();
    expect(useComparisonStore.getState().page).toBe(1);
  });
});

describe("page-local decisions", () => {
  it("retains the selection, trashes only the visible complement, and fills the page", async () => {
    useComparisonStore.getState().selectSlot(1, "toggle");

    const result = await useComparisonStore
      .getState()
      .requestPageDecision(false, false);

    expect(result).toEqual({ kind: "continued" });
    const deleted = invokeCalls.find((call) => call.command === "delete_items")
      ?.args.items as Array<{ hash: string }>;
    expect(deleted.map((item) => item.hash)).toEqual(["h2", "h3"]);
    expect(
      useComparisonStore.getState().members.map((item) => item.hash),
    ).toEqual(["h4", "h5", "h6", "h7"]);
  });

  it("completes an all-selected page without a filesystem operation", async () => {
    useComparisonStore.getState().selectAll();
    const result = await useComparisonStore
      .getState()
      .requestPageDecision(false, false);

    expect(result).toEqual({ kind: "continued" });
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
    expect(
      useComparisonStore.getState().members.map((item) => item.hash),
    ).toEqual(["h4", "h5", "h6", "h7"]);
  });

  it("does nothing with no selection and explains why", async () => {
    useComparisonStore.setState({
      selected: new Set(),
      anchor: null,
      anchors: new Set(),
    });
    expect(
      await useComparisonStore.getState().requestPageDecision(false, false),
    ).toBeNull();
    expect(useComparisonStore.getState().message).toBe(
      "Select at least one image to keep.",
    );
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
  });

  it("offers a separate explicit Trash-all action", async () => {
    await useComparisonStore.getState().requestPageDecision(false, false, true);
    const deleted = invokeCalls.find((call) => call.command === "delete_items")
      ?.args.items as Array<{ hash: string }>;
    expect(deleted.map((item) => item.hash)).toEqual(["h0", "h1", "h2", "h3"]);
  });

  it("always confirms a permanent page decision", async () => {
    expect(
      await useComparisonStore.getState().requestPageDecision(true, false),
    ).toBeNull();
    expect(useComparisonStore.getState().pendingAction?.permanent).toBe(true);
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
  });

  it("preserves the draft when confirmation is cancelled", async () => {
    useComparisonStore.getState().selectSlot(1, "toggle");
    await useComparisonStore.getState().requestPageDecision(false, true);
    useComparisonStore.getState().cancelPendingAction();
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h0", "h1"]),
    );
    expect(useComparisonStore.getState().members).toHaveLength(8);
  });
});

describe("partial deletion", () => {
  it("removes successes and retainers while keeping only failed targets retryable", async () => {
    openSession(4);
    mockCommands({
      delete_items: ({ items }) => {
        const requested = items as Array<{
          hash: string | null;
          pathId: number | null;
        }>;
        return {
          cancelled: true,
          error: null,
          failedFiles: 1,
          items: [
            { item: requested[0], failedFiles: 0 },
            { item: requested[1], failedFiles: 1 },
          ],
        };
      },
    });

    expect(
      await useComparisonStore.getState().requestPageDecision(false, false),
    ).toEqual({ kind: "failed" });
    const state = useComparisonStore.getState();
    expect(state.members.map((item) => item.hash)).toEqual(["h2", "h3"]);
    expect(state.failure?.targetHashes).toEqual(["h2", "h3"]);
    expect(state.selected).toEqual(new Set(["h2", "h3"]));
  });

  it("reconfirms a retry when current policy requires it", async () => {
    useComparisonStore.setState({
      failure: {
        kind: "page",
        permanent: false,
        keepHashes: [],
        targetHashes: ["h1"],
        message: "failed",
      },
    });
    await useComparisonStore.getState().retryFailure(true);
    expect(useComparisonStore.getState().pendingAction?.targetHashes).toEqual([
      "h1",
    ]);
  });
});

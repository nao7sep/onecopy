import { beforeEach, describe, expect, it } from "vitest";
import {
  useComparisonStore,
  type GroupMember,
} from "../../src/state/comparison-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import { message } from "../../src/i18n/translate";

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
    set_window_fullscreen: () => null,
    set_spread_fullscreen: () => null,
  });
  openSession(8);
});

describe("draft page selection", () => {
  it("retains each undecided page draft while browsing", () => {
    useComparisonStore.getState().selectSlot(1, "toggle");
    useComparisonStore.getState().nextPage();
    useComparisonStore.getState().selectSlot(2, "toggle");
    useComparisonStore.getState().prevPage();

    const state = useComparisonStore.getState();
    expect(state.selected).toEqual(new Set(["h1", "h6"]));
    expect(state.anchor).toBe("h1");
  });

  it("preserves a deliberately empty page draft", () => {
    expect(useComparisonStore.getState().selected).toEqual(new Set());

    useComparisonStore.getState().nextPage();
    useComparisonStore.getState().prevPage();

    const state = useComparisonStore.getState();
    expect(state.selected).toEqual(new Set());
    expect(state.anchor).toBe("h0");
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
  it("reviews the marked keepers and visible complement before acting", async () => {
    useComparisonStore.getState().selectSlot(1, "toggle");

    const result = await useComparisonStore
      .getState()
      .requestPageDecision(false);

    expect(result).toBeNull();
    expect(useComparisonStore.getState().pendingAction).toMatchObject({
      keepHashes: ["h1"],
      targetHashes: ["h0", "h2", "h3"],
    });
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
    expect(
      await useComparisonStore.getState().confirmPendingAction(),
    ).toEqual({ kind: "continued" });
    const deleted = invokeCalls.find((call) => call.command === "delete_items")
      ?.args.items as Array<{ hash: string }>;
    expect(deleted.map((item) => item.hash)).toEqual(["h0", "h2", "h3"]);
    expect(
      useComparisonStore.getState().members.map((item) => item.hash),
    ).toEqual(["h4", "h5", "h6", "h7"]);
  });

  // With marks, Enter opens an exact-count review before requesting
  // recoverable deletion of every other image on the current visible page,
  // always, even when only one image is targeted (it is never skipped the way
  // a single-item Delete elsewhere can be).
  it("still reviews a single-image visible complement, never skipping straight to deletion", async () => {
    useComparisonStore.getState().selectSlot(0, "toggle");
    useComparisonStore.getState().selectSlot(1, "toggle");
    useComparisonStore.getState().selectSlot(2, "toggle");

    const result = await useComparisonStore.getState().requestPageDecision(false);

    expect(result).toBeNull();
    expect(useComparisonStore.getState().pendingAction).toMatchObject({
      keepHashes: ["h0", "h1", "h2"],
      targetHashes: ["h3"],
    });
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(false);
  });

  it("completes an all-selected page without a filesystem operation", async () => {
    useComparisonStore.getState().markAll();
    const result = await useComparisonStore
      .getState()
      .requestPageDecision(false);

    expect(result).toEqual({ kind: "continued" });
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
    expect(
      useComparisonStore.getState().members.map((item) => item.hash),
    ).toEqual(["h4", "h5", "h6", "h7"]);
  });

  it("completes an all-selected page with Shift as well, since nothing would be deleted", async () => {
    useComparisonStore.getState().markAll();
    const result = await useComparisonStore.getState().requestPageDecision(true);

    expect(result).toEqual({ kind: "continued" });
    expect(useComparisonStore.getState().pendingAction).toBeNull();
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(false);
  });

  it("offers a separate explicit Trash-all action", async () => {
    await useComparisonStore.getState().requestPageDecision(false, true);
    expect(useComparisonStore.getState().pendingAction?.targetHashes).toEqual([
      "h0",
      "h1",
      "h2",
      "h3",
    ]);
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
    await useComparisonStore.getState().confirmPendingAction();
    const deleted = invokeCalls.find((call) => call.command === "delete_items")
      ?.args.items as Array<{ hash: string }>;
    expect(deleted.map((item) => item.hash)).toEqual(["h0", "h1", "h2", "h3"]);
  });

  it("always confirms a permanent page decision", async () => {
    useComparisonStore.getState().selectSlot(0, "toggle");
    expect(
      await useComparisonStore.getState().requestPageDecision(true),
    ).toBeNull();
    expect(useComparisonStore.getState().pendingAction?.permanent).toBe(true);
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
  });

  it("preserves the draft when confirmation is cancelled", async () => {
    useComparisonStore.getState().selectSlot(1, "toggle");
    await useComparisonStore.getState().requestPageDecision(false);
    useComparisonStore.getState().cancelPendingAction();
    expect(useComparisonStore.getState().selected).toEqual(
      new Set(["h1"]),
    );
    expect(useComparisonStore.getState().members).toHaveLength(8);
  });
});

// Delete/Backspace trashes only the visible
// keep-marked images, never a mark left on an unseen page; a single visible
// mark trashes directly when the confirm-single-item preference is off, more
// than one visible mark always reviews, and Shift always forces a permanent,
// always-confirmed review regardless of that preference.
describe("selection deletion (Delete/Backspace)", () => {
  it("trashes a single visible mark directly when the confirm preference is off", async () => {
    useComparisonStore.getState().selectSlot(1, "toggle");

    const result = await useComparisonStore.getState().requestSelectionDelete(false, false);

    expect(result).toEqual({ kind: "continued" });
    expect(useComparisonStore.getState().pendingAction).toBeNull();
    const deleted = invokeCalls.find((call) => call.command === "delete_items")
      ?.args.items as Array<{ hash: string }>;
    expect(deleted.map((item) => item.hash)).toEqual(["h1"]);
    expect(
      useComparisonStore.getState().members.map((item) => item.hash),
    ).toEqual(["h0", "h2", "h3", "h4", "h5", "h6", "h7"]);
  });

  it("reviews more than one visible mark even when the confirm preference is off", async () => {
    useComparisonStore.getState().selectSlot(1, "toggle");
    useComparisonStore.getState().selectSlot(2, "toggle");

    const result = await useComparisonStore.getState().requestSelectionDelete(false, false);

    expect(result).toBeNull();
    expect(useComparisonStore.getState().pendingAction).toMatchObject({
      kind: "selection",
      permanent: false,
      targetHashes: ["h1", "h2"],
    });
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(false);
  });

  it("always forces a permanent, confirmed review for Shift+Delete, even for one mark", async () => {
    useComparisonStore.getState().selectSlot(1, "toggle");

    const result = await useComparisonStore.getState().requestSelectionDelete(true, false);

    expect(result).toBeNull();
    expect(useComparisonStore.getState().pendingAction).toMatchObject({
      permanent: true,
      targetHashes: ["h1"],
    });
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(false);
  });

  it("trashes only the mark visible on the current page, leaving a mark on another page untouched", async () => {
    useComparisonStore.getState().selectSlot(1, "toggle"); // marks h1 on page 0
    useComparisonStore.getState().nextPage();
    useComparisonStore.getState().selectSlot(2, "toggle"); // marks h6 on page 1

    const result = await useComparisonStore.getState().requestSelectionDelete(false, false);

    expect(result).toEqual({ kind: "continued" });
    const deleted = invokeCalls.find((call) => call.command === "delete_items")
      ?.args.items as Array<{ hash: string }>;
    expect(deleted.map((item) => item.hash)).toEqual(["h6"]);
    const state = useComparisonStore.getState();
    expect(state.members.map((item) => item.hash)).toEqual([
      "h0", "h1", "h2", "h3", "h4", "h5", "h7",
    ]);
    expect(state.selected.has("h1")).toBe(true);
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

    useComparisonStore.getState().selectSlot(0, "toggle");
    await useComparisonStore.getState().requestPageDecision(false);

    expect(
      await useComparisonStore.getState().confirmPendingAction(),
    ).toEqual({ kind: "failed" });
    const state = useComparisonStore.getState();
    expect(state.members.map((item) => item.hash)).toEqual(["h2", "h3"]);
    expect(state.failure?.targetHashes).toEqual(["h2", "h3"]);
    expect(state.selected).toEqual(new Set());
  });

  it("reconfirms a retry when current policy requires it", async () => {
    useComparisonStore.setState({
      failure: {
        kind: "page",
        permanent: false,
        keepHashes: [],
        targetHashes: ["h1"],
        message: message("comparison.deleteStartFailedRetry"),
      },
    });
    await useComparisonStore.getState().retryFailure(true);
    expect(useComparisonStore.getState().pendingAction?.targetHashes).toEqual([
      "h1",
    ]);
  });
});

// The paged comparison (Phase 33, replacing the turn/pin/refill model).
//
// Viewing and deciding are decoupled: pages are a viewport, marks are the
// only state, and ONE commit decides the whole group. The safety rule under
// every spec here: nothing from an unvisited page can ever be deleted —
// guaranteed structurally, because Enter advances through unseen pages
// before it will commit.

import { beforeEach, describe, expect, it } from "vitest";
import {
  nextUnseenPage,
  pageCountOf,
  useComparisonStore,
  visibleSlots,
} from "../../src/state/comparison-store";
import { useItemsStore } from "../../src/state/items-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

function member(i: number) {
  return {
    hash: `h${i}`,
    fileName: `IMG_${i}.jpg`,
    width: 100,
    height: 100,
    byteSize: 1000,
    sharpness: null,
    faceScore: null,
    copyCount: 1,
    hasThumb: true,
  };
}

/** A session over `count` members with a page size of `perPage`. */
function openSession(count: number, perPage: number): void {
  const members = Array.from({ length: count }, (_, i) => member(i));
  useComparisonStore.setState({
    open: true,
    members,
    kept: new Set(),
    visited: new Set([0]),
    page: 0,
    shortlist: false,
    shortlistPage: 0,
    sessionMembers: members.map((m) => m.hash),
    capacities: [perPage],
    busy: false,
    commitFailure: null,
    permanentArmed: true,
    pendingPermanentCommit: false,
    pendingCommit: null,
  });
}

function deleted(): unknown[] {
  return invokeCalls
    .filter((c) => c.command === "delete_items")
    .flatMap((c) => c.args.items as Array<{ hash: string | null }>)
    .map((item) => item.hash);
}

beforeEach(() => {
  resetTauriMocks();
  mockCommands({
    delete_items: ({ items }) => ({
      cancelled: false,
      error: null,
      failedFiles: 0,
      items: (items as Array<{ hash: string | null; pathId: number | null }>).map((item) => ({
        item,
        failedFiles: 0,
      })),
    }),
    get_section_items: () => [],
    get_section_counts: () => [],
    patch_state: () => ({}),
    get_item_detail: () => null,
  });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: [],
    selectedItem: null,
    selectedKeys: new Set(),
  });
});

describe("Enter's rhythm: advance, then commit", () => {
  it("never deletes while pages remain unseen", async () => {
    openSession(12, 4); // 3 pages
    useComparisonStore.getState().toggleKeep(0); // mark h0 on page 1

    await useComparisonStore.getState().commitTurn(false);
    expect(deleted()).toHaveLength(0);
    expect(useComparisonStore.getState().page).toBe(1);

    await useComparisonStore.getState().commitTurn(false);
    expect(deleted()).toHaveLength(0);
    expect(useComparisonStore.getState().page).toBe(2);
  });

  it("marks persist across pages and one commit decides the whole group", async () => {
    openSession(12, 4);
    useComparisonStore.getState().toggleKeep(1); // h1 on page 1
    await useComparisonStore.getState().commitTurn(false); // -> page 2
    useComparisonStore.getState().toggleKeep(2); // h6 on page 2
    await useComparisonStore.getState().commitTurn(false); // -> page 3
    await useComparisonStore.getState().commitTurn(false); // all seen -> commit

    // Multi-page trashing confirms with the counts before anything moves.
    const pending = useComparisonStore.getState().pendingCommit;
    expect(deleted()).toHaveLength(0);
    expect(pending).toEqual({ keepCount: 2, trashCount: 10, permanent: false });

    await useComparisonStore.getState().confirmPendingCommit();
    expect(deleted()).toHaveLength(10);
    expect(deleted()).not.toContain("h1");
    expect(deleted()).not.toContain("h6");
    expect(useComparisonStore.getState().open).toBe(false);
  });

  it("free navigation marks pages as seen, so Enter can go straight to commit", async () => {
    openSession(8, 4);
    useComparisonStore.getState().toggleKeep(0);
    useComparisonStore.getState().nextPage(); // sees page 2 by arrow
    useComparisonStore.getState().prevPage(); // back — both seen now

    await useComparisonStore.getState().commitTurn(false);
    // All pages visited: this Enter is the COMMIT (multi-page -> confirm).
    expect(useComparisonStore.getState().pendingCommit).not.toBeNull();
  });
});

describe("the commit confirmations", () => {
  it("a single-page group with a mark commits instantly — two keystrokes, no dialog", async () => {
    openSession(4, 16);
    useComparisonStore.getState().toggleKeep(0);

    await useComparisonStore.getState().commitTurn(false);

    expect(useComparisonStore.getState().pendingCommit).toBeNull();
    expect(deleted().sort()).toEqual(["h1", "h2", "h3"]);
  });

  it("zero marks means trash ALL — possible at last, and always confirmed", async () => {
    // The turn model could not express "all 12 are bad" at all: goners were
    // only computed when something was kept.
    openSession(4, 16);

    await useComparisonStore.getState().commitTurn(false);
    expect(deleted()).toHaveLength(0);
    expect(useComparisonStore.getState().pendingCommit).toEqual({
      keepCount: 0,
      trashCount: 4,
      permanent: false,
    });

    await useComparisonStore.getState().confirmPendingCommit();
    expect(deleted().sort()).toEqual(["h0", "h1", "h2", "h3"]);
  });

  it("keeping everything commits nothing and just finishes", async () => {
    openSession(2, 16);
    useComparisonStore.getState().toggleKeep(0);
    useComparisonStore.getState().toggleKeep(1);

    await useComparisonStore.getState().commitTurn(false);

    expect(deleted()).toHaveLength(0);
    expect(useComparisonStore.getState().open).toBe(false);
  });

  it("the permanent arming still asks once per session", async () => {
    openSession(3, 16);
    useComparisonStore.setState({ permanentArmed: false });
    useComparisonStore.getState().toggleKeep(0);

    await useComparisonStore.getState().commitTurn(true);
    expect(deleted()).toHaveLength(0);
    expect(useComparisonStore.getState().pendingPermanentCommit).toBe(true);
  });
});

describe("partial commit recovery", () => {
  it("retires successes and retries only logical items that remain", async () => {
    openSession(3, 16);
    useComparisonStore.getState().toggleKeep(0);
    let h2Attempts = 0;
    mockCommands({
      delete_items: ({ items }) => {
        const results = (items as Array<{ hash: string; pathId: null }>).map((item) => {
          const failedFiles = item.hash === "h2" && h2Attempts++ === 0 ? 1 : 0;
          return { item, failedFiles };
        });
        return {
          cancelled: false,
          error: null,
          failedFiles: results.reduce((total, result) => total + result.failedFiles, 0),
          items: results,
        };
      },
      get_issues: () => ({ total: 1, rows: [] }),
    });

    await useComparisonStore.getState().commitTurn(false);

    expect(deleted()).toEqual(["h1", "h2"]);
    expect(useComparisonStore.getState().open).toBe(true);
    expect(useComparisonStore.getState().members.map((m) => m?.hash ?? null)).toEqual([
      "h0",
      null,
      "h2",
    ]);
    expect(useComparisonStore.getState().commitFailure?.message).toContain(
      "Retry targets only the remaining items",
    );

    await useComparisonStore.getState().commitTurn(false);

    expect(deleted()).toEqual(["h1", "h2", "h2"]);
    expect(useComparisonStore.getState().open).toBe(false);
  });
});

describe("an active commit owns the comparison decision state", () => {
  it("cannot close, page, or change keepers before the batch reaches a boundary", async () => {
    openSession(8, 4);
    useComparisonStore.setState({ busy: true });

    useComparisonStore.getState().toggleKeep(0);
    useComparisonStore.getState().nextPage();
    await useComparisonStore.getState().close();

    const state = useComparisonStore.getState();
    expect(state.open).toBe(true);
    expect(state.page).toBe(0);
    expect(state.kept.size).toBe(0);
  });
});

describe("Escape", () => {
  it("leaves with nothing deleted and marks discarded", async () => {
    openSession(8, 4);
    useComparisonStore.getState().toggleKeep(0);
    useComparisonStore.getState().close();

    expect(deleted()).toHaveLength(0);
    const state = useComparisonStore.getState();
    expect(state.open).toBe(false);
    expect(state.kept.size).toBe(0);
    expect(state.members).toHaveLength(0);
  });
});

describe("the shortlist", () => {
  it("shows exactly the marks, and unmarking there shrinks it", () => {
    openSession(8, 4);
    useComparisonStore.getState().toggleKeep(0); // h0
    useComparisonStore.getState().toggleKeep(2); // h2
    useComparisonStore.getState().toggleShortlist();

    let visible = visibleSlots(useComparisonStore.getState());
    expect(visible.map((m) => m?.hash)).toEqual(["h0", "h2"]);

    // Slot keys act on the VISIBLE page — in the shortlist, slot 2 is h2.
    useComparisonStore.getState().toggleKeep(1);
    visible = visibleSlots(useComparisonStore.getState());
    expect(visible.map((m) => m?.hash)).toEqual(["h0"]);
  });

  it("committing from the shortlist still requires every page seen", async () => {
    openSession(8, 4); // 2 pages, only page 1 seen
    useComparisonStore.getState().toggleKeep(0);
    useComparisonStore.getState().toggleShortlist();

    await useComparisonStore.getState().commitTurn(false);

    // The commit attempt dropped back to the pages and advanced instead.
    expect(deleted()).toHaveLength(0);
    expect(useComparisonStore.getState().shortlist).toBe(false);
    expect(useComparisonStore.getState().page).toBe(1);
  });
});

describe("the page math", () => {
  it("pageCountOf covers the edges", () => {
    expect(pageCountOf(0, 12)).toBe(1);
    expect(pageCountOf(12, 12)).toBe(1);
    expect(pageCountOf(13, 12)).toBe(2);
    expect(pageCountOf(300, 12)).toBe(25);
  });

  it("nextUnseenPage walks forward with wrap and reports done", () => {
    expect(nextUnseenPage(new Set([0]), 0, 3)).toBe(1);
    expect(nextUnseenPage(new Set([0, 1]), 1, 3)).toBe(2);
    // Wrap: standing on the last page with page 1 unseen.
    expect(nextUnseenPage(new Set([0, 2]), 2, 3)).toBe(1);
    expect(nextUnseenPage(new Set([0, 1, 2]), 2, 3)).toBeNull();
  });
});

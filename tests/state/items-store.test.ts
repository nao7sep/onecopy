// The grid's selection state machine.
//
// This store drives every keystroke of the culling workflow — the anchor the
// metadata pane and preview follow, the multi-selection the bulk actions read,
// and the recovery that lands after a delete. The specs below assert the
// behaviour a user feels at the keyboard, not the shape of the state object.

import { beforeEach, describe, expect, it } from "vitest";
import { useItemsStore } from "../../src/state/items-store";
import type { SectionItem } from "../../src/models/items";
import {
  invokeCalls,
  mockCommand,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

function item(over: Partial<SectionItem> & { pathId: number }): SectionItem {
  return {
    hash: `h${over.pathId}`,
    fileName: `IMG_${over.pathId}.jpg`,
    resolvedUtcMs: over.pathId * 1000,
    copyCount: 1,
    width: 100,
    height: 100,
    hasThumb: true,
    similarGroupId: null,
    sharpness: null,
    byteSize: 1000,
    hasCompanions: false,
    durationMs: null,
    namesDiffer: false,
    dirPaths: ["/photos"],
    ...over,
  };
}

const SECTION = { kind: "image" as const, month: "2026-01" };

/** Seed the store with a loaded section, bypassing the load path. */
function seed(items: SectionItem[]): void {
  useItemsStore.setState({
    selected: SECTION,
    items,
    loading: false,
    selectedItem: null,
    selectedKeys: new Set(),
    detail: null,
    sortOrders: { media: { order: "time", desc: false }, other: { order: "name", desc: false } },
  });
}

beforeEach(() => {
  resetTauriMocks();
  useItemsStore.setState({
    selected: null,
    items: [],
    loading: false,
    selectedItem: null,
    selectedKeys: new Set(),
    detail: null,
    sortOrders: { media: { order: "time", desc: false }, other: { order: "name", desc: false } },
  });
  // Anchor moves fan out to these; none is under test here.
  mockCommands({
    patch_state: () => ({}),
    get_item_detail: () => null,
    delete_item: () => ({ deletedFiles: 1, failedFiles: 0 }),
    get_section_counts: () => [],
  });
});

describe("refresh", () => {
  it("keeps the anchor visible for the whole reload", async () => {
    const items = [item({ pathId: 1 }), item({ pathId: 2 }), item({ pathId: 3 })];
    seed(items);
    useItemsStore.getState().selectItem("h2");

    // A deferred backend response models the real IPC round trip, which on a
    // large month runs into the hundreds of milliseconds.
    let release!: (value: SectionItem[]) => void;
    mockCommand(
      "get_section_items",
      () => new Promise<SectionItem[]>((resolve) => (release = resolve)),
    );

    const pending = useItemsStore.getState().refresh();
    // Mid-flight: an arrow key or a Delete lands HERE. The anchor must still
    // be the user's photo, not null — a null anchor makes the grid select
    // item zero and makes Delete a silent no-op.
    expect(useItemsStore.getState().selectedItem).toBe("h2");
    expect(useItemsStore.getState().selectedKeys.has("h2")).toBe(true);

    release(items);
    await pending;
    expect(useItemsStore.getState().selectedItem).toBe("h2");
  });

  it("drops an anchor that did not survive the reload", async () => {
    const items = [item({ pathId: 1 }), item({ pathId: 2 })];
    seed(items);
    useItemsStore.getState().selectItem("h2");
    mockCommand("get_section_items", () => [item({ pathId: 1 })]);

    await useItemsStore.getState().refresh();

    expect(useItemsStore.getState().selectedItem).toBeNull();
    expect(useItemsStore.getState().selectedKeys.size).toBe(0);
  });

  it("does not revert a selection made while it was in flight", async () => {
    const items = [item({ pathId: 1 }), item({ pathId: 2 })];
    seed(items);
    useItemsStore.getState().selectItem("h1");

    let release!: (value: SectionItem[]) => void;
    mockCommand(
      "get_section_items",
      () => new Promise<SectionItem[]>((resolve) => (release = resolve)),
    );

    const pending = useItemsStore.getState().refresh();
    useItemsStore.getState().selectItem("h2");
    release(items);
    await pending;

    expect(useItemsStore.getState().selectedItem).toBe("h2");
  });
});

describe("a failed load", () => {
  it("does not blank a section the user already moved on to", async () => {
    seed([]);
    let failMarch!: (reason: Error) => void;
    mockCommand("get_section_items", (args) => {
      if (args.month === "2026-03") {
        return new Promise((_resolve, reject) => (failMarch = reject));
      }
      return [item({ pathId: 1 }), item({ pathId: 2 }), item({ pathId: 3 })];
    });

    const march = useItemsStore
      .getState()
      .select({ kind: "image", month: "2026-03" });
    const april = useItemsStore
      .getState()
      .select({ kind: "image", month: "2026-04" });
    await april;
    failMarch(new Error("march is gone"));
    await march;

    // April's three items must survive March's rejection — the catch writes
    // unconditionally today while the success path is guarded.
    expect(useItemsStore.getState().items).toHaveLength(3);
  });
});

describe("incremental derived rows", () => {
  it("updates facts and remaps every selection key across identity promotion", () => {
    seed([item({ pathId: 1, hash: "quick-1", hasThumb: false })]);
    useItemsStore.setState({
      selectedItem: "quick-1",
      selectedKeys: new Set(["quick-1"]),
      rangeOrigin: "quick-1",
      rangeBase: new Set(["quick-1"]),
    });

    useItemsStore.getState().applyDerivedItem(
      "quick-1",
      item({ pathId: 1, hash: "real", hasThumb: true, width: 4000 }),
    );

    const state = useItemsStore.getState();
    expect(state.items).toHaveLength(1);
    expect(state.items[0]).toMatchObject({ hash: "real", hasThumb: true, width: 4000 });
    expect(state.selectedItem).toBe("real");
    expect([...state.selectedKeys]).toEqual(["real"]);
    expect(state.rangeOrigin).toBe("real");
    expect([...state.rangeBase]).toEqual(["real"]);
  });
});

describe("rangeSelect", () => {
  const keys = ["h1", "h2", "h3", "h4", "h5", "h6"];

  beforeEach(() => {
    seed(keys.map((_k, i) => item({ pathId: i + 1 })));
  });

  it("selects the span between the anchor and the target", () => {
    useItemsStore.getState().selectItem("h2");
    useItemsStore.getState().rangeSelect(keys, "h5");
    expect([...useItemsStore.getState().selectedKeys].sort()).toEqual([
      "h2",
      "h3",
      "h4",
      "h5",
    ]);
  });

  it("replaces the range on a second shift-click instead of only growing", () => {
    useItemsStore.getState().selectItem("h1");
    useItemsStore.getState().rangeSelect(keys, "h6");
    expect(useItemsStore.getState().selectedKeys.size).toBe(6);

    // Narrowing is the standard gesture: shift-clicking nearer the origin
    // means "I overshot, take it back", not "keep everything".
    useItemsStore.getState().rangeSelect(keys, "h3");
    expect([...useItemsStore.getState().selectedKeys].sort()).toEqual([
      "h1",
      "h2",
      "h3",
    ]);
  });

  it("shrinks when shift+arrow reverses direction", () => {
    useItemsStore.getState().selectItem("h1");
    useItemsStore.getState().rangeSelect(keys, "h2");
    useItemsStore.getState().rangeSelect(keys, "h3");
    expect(useItemsStore.getState().selectedKeys.size).toBe(3);

    useItemsStore.getState().rangeSelect(keys, "h2");
    expect([...useItemsStore.getState().selectedKeys].sort()).toEqual([
      "h1",
      "h2",
    ]);
  });
});

describe("deleteSelected", () => {
  it("recovers onto the next item in DISPLAY order, not backend order", async () => {
    // Name order is deliberately the reverse of time order, so a recovery
    // that walks the backend array lands somewhere the user is not looking.
    const items = [
      item({ pathId: 1, fileName: "c.jpg" }),
      item({ pathId: 2, fileName: "b.jpg" }),
      item({ pathId: 3, fileName: "a.jpg" }),
    ];
    seed(items);
    useItemsStore.setState({ sortOrders: { media: { order: "name", desc: false }, other: { order: "name", desc: false } } });
    mockCommand("get_section_items", () =>
      items.filter((i) => i.hash !== "h2"),
    );

    useItemsStore.getState().selectItem("h2"); // "b.jpg", middle in name order
    await useItemsStore.getState().deleteSelected(false);

    // Display order is a.jpg(h3), b.jpg(h2), c.jpg(h1) — the next after b is c.
    expect(useItemsStore.getState().selectedItem).toBe("h1");
  });

  it("recovers onto the previous item when the last one is deleted", async () => {
    const items = [item({ pathId: 1 }), item({ pathId: 2 }), item({ pathId: 3 })];
    seed(items);
    mockCommand("get_section_items", () =>
      items.filter((i) => i.hash !== "h3"),
    );

    useItemsStore.getState().selectItem("h3");
    await useItemsStore.getState().deleteSelected(false);

    expect(useItemsStore.getState().selectedItem).toBe("h2");
  });

  it("falls back to the most recently selected item when the anchor toggles off", async () => {
    const items = [
      item({ pathId: 1 }),
      item({ pathId: 2 }),
      item({ pathId: 3 }),
      item({ pathId: 4 }),
    ];
    seed(items);
    mockCommand("get_section_items", () => items);

    // Click h2, ctrl-click h3, ctrl-click h4, then take h4 back. The anchor
    // must land on h3 — the most recently selected REMAINING item — so the
    // user always sees which photo a multi-select is "on". It previously went
    // null, leaving the preview and metadata pane pointing at nothing while
    // two photos were still selected.
    useItemsStore.getState().selectItem("h2");
    useItemsStore.getState().toggleItem("h3");
    useItemsStore.getState().toggleItem("h4");
    useItemsStore.getState().toggleItem("h4");
    expect(useItemsStore.getState().selectedItem).toBe("h3");

    // And taking h3 back too steps to the one before it.
    useItemsStore.getState().toggleItem("h3");
    expect(useItemsStore.getState().selectedItem).toBe("h2");
  });

  it("deletes every selected item and nothing else", async () => {
    const items = [item({ pathId: 1 }), item({ pathId: 2 }), item({ pathId: 3 })];
    seed(items);
    mockCommand("get_section_items", () => [item({ pathId: 2 })]);

    useItemsStore.getState().selectItem("h1");
    useItemsStore.getState().toggleItem("h3");
    await useItemsStore.getState().deleteSelected(false);

    const deleted = invokeCalls
      .filter((c) => c.command === "delete_item")
      .map((c) => c.args.hash);
    expect(deleted.sort()).toEqual(["h1", "h3"]);
  });

  it("surfaces copies that failed to delete", async () => {
    const items = [item({ pathId: 1 }), item({ pathId: 2 })];
    seed(items);
    mockCommand("get_section_items", () => items);
    mockCommand("delete_item", () => ({ deletedFiles: 0, failedFiles: 1 }));

    useItemsStore.getState().selectItem("h1");
    await useItemsStore.getState().deleteSelected(false);

    // A delete that silently did nothing is the worst outcome: the user
    // presses again, and again. Something must reach the UI.
    expect(useItemsStore.getState().message).toBeTruthy();
  });

  it("surfaces a rejected delete instead of only logging it", async () => {
    const items = [item({ pathId: 1 })];
    seed(items);
    mockCommand("get_section_items", () => items);
    mockCommand("delete_item", () => {
      throw new Error("a source volume is not present");
    });

    useItemsStore.getState().selectItem("h1");
    await useItemsStore.getState().deleteSelected(false);

    expect(useItemsStore.getState().message).toMatch(/not present/i);
  });
});

describe("out-of-order responses (the reads are async commands now)", () => {
  // The stores used to lean on main-thread commands serializing FIFO. On the
  // async runtime two reloads race, and without the sequence guard the OLDER
  // response landing last resurrects rows a newer reload already dropped —
  // deleted photos reappearing until the next refresh.
  it("a stale section reload cannot overwrite a fresher one", async () => {
    const resolvers: ((items: SectionItem[]) => void)[] = [];
    mockCommand(
      "get_section_items",
      () => new Promise<SectionItem[]>((resolve) => resolvers.push(resolve)),
    );
    const section = { kind: "image" as const, month: "2026-01" };
    useItemsStore.setState({ selected: section });

    const older = useItemsStore.getState().select(section);
    const newer = useItemsStore.getState().refresh(); // same section OBJECT
    expect(resolvers).toHaveLength(2);

    // The newer reload returns first — post-delete, one row gone.
    resolvers[1]([item({ pathId: 2 })]);
    await newer;
    // The OLDER response straggles in with the pre-delete rows.
    resolvers[0]([item({ pathId: 1 }), item({ pathId: 2 })]);
    await older;

    expect(useItemsStore.getState().items.map((i) => i.pathId)).toEqual([2]);
  });

  it("a stale detail response for the same anchor is discarded", async () => {
    const resolvers: ((detail: unknown) => void)[] = [];
    mockCommand(
      "get_item_detail",
      () => new Promise((resolve) => resolvers.push(resolve)),
    );
    useItemsStore.setState({ items: [item({ pathId: 1 })], selected: { kind: "image", month: "2026-01" } });

    useItemsStore.getState().selectItem("h1");
    useItemsStore.getState().selectItem("h1"); // re-select fires a second fetch
    // Macrotask drains, not bare microtask ticks: the response runs through
    // invoke's async wrapper AND the .then chain, and a single microtask left
    // the stale write still pending — the assertion passed with the guard
    // deleted. setTimeout(0) flushes the whole chain (mutation-verified).
    const drain = () => new Promise((resolve) => setTimeout(resolve, 0));
    await drain();
    expect(resolvers).toHaveLength(2);

    resolvers[1]({ fileName: "IMG_1.jpg", copyPaths: ["/b"], companionPaths: [] });
    await drain();
    resolvers[0]({ fileName: "IMG_1.jpg", copyPaths: ["/a"], companionPaths: [] });
    await drain();

    // The newer fetch's answer stands.
    expect(
      (useItemsStore.getState().detail as { copyPaths: string[] } | null)?.copyPaths,
    ).toEqual(["/b"]);
  });
});

describe("the direction toggle (Phase 33)", () => {
  it("re-picking the active order flips it; a fresh order starts natural", () => {
    useItemsStore.setState({
      selected: { kind: "image", month: "2026-01" },
      sortOrders: {
        media: { order: "time", desc: false },
        other: { order: "name", desc: false },
      },
    });
    useItemsStore.getState().setSortOrder("time");
    expect(useItemsStore.getState().sortOrders.media).toEqual({ order: "time", desc: true });
    // A fresh order arrives in its NATURAL direction (size = biggest first),
    // not whatever direction the previous order left behind.
    useItemsStore.getState().setSortOrder("size");
    expect(useItemsStore.getState().sortOrders.media).toEqual({ order: "size", desc: true });
    // And only the active lane moved.
    expect(useItemsStore.getState().sortOrders.other).toEqual({ order: "name", desc: false });
  });
});

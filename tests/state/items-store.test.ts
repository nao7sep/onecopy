// @vitest-environment happy-dom
//
// Failures are recorded through the notifications store, which renders the
// sentence in the language the document declares, so this spec needs a document
// even though the subject is not the interface.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { useItemsStore } from "../../src/state/items-store";
import { currentMainFeedback, useMainFeedbackStore } from "../../src/state/main-feedback-store";
import { captureDeleteSelection, deleteItems } from "../../src/workflows/items";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import {
  invokeCalls,
  mockCommand,
  mockCommands,
  mockSectionItems,
  resetTauriMocks,
} from "../mocks/tauri";

function item(pathId: number, over: Partial<SectionItem> = {}): SectionItem {
  return {
    hash: `h${pathId}`,
    pathId,
    fileName: `IMG_${String(pathId).padStart(4, "0")}.jpg`,
    resolvedUtcMs: pathId * 1000,
    copyCount: 1,
    width: 100,
    height: 100,
    hasThumb: true,
    similarGroupId: null,
    similarCount: 0,
    sharpness: null,
    faceScore: null,
    byteSize: 1000,
    hasCompanions: false,
    durationMs: null,
    dirPaths: ["/photos"],
    derivedWork: EMPTY_ITEM_WORK,
    ...over,
  };
}

const SECTION = { kind: "image" as const, month: "2026-01" };

function resetStore(): void {
  useItemsStore.setState({
    selected: null,
    items: [],
    totalItems: 0,
    windowStart: 0,
    itemPositions: new Map(),
    reconciliationId: 0,
    loading: false,
    loadError: null,
    selectedItem: null,
    selectedKeys: new Set(),
    selectedPositions: new Map(),
    rangeOrigin: null,
    rangeOriginPosition: null,
    rangeBase: new Set(),
    rangeBasePositions: new Map(),
    sectionMemory: {},
    currentContext: null,
    scrollRequest: null,
    detail: null,
    sortOrders: {
      media: { order: "time", desc: false },
      other: { order: "name", desc: false },
    },
  });
}

function mockSection(rows: SectionItem[] | (() => SectionItem[] | Promise<SectionItem[]>)): void {
  mockSectionItems(() =>
    typeof rows === "function" ? rows() : rows,
  );
}

beforeEach(() => {
  resetTauriMocks();
  resetStore();
  mockCommands({
    activity_record: () => null,
    record_recent_notification: () => ({}),
    patch_state: () => ({}),
    get_item_detail: () => ({ fileName: "item", kind: "image" }),
    get_section_counts: () => [],
    delete_items: () => ({ error: null, failedFiles: 0 }),
  });
});

describe("bounded section state", () => {
  it("reveals a distant diagnostic target atomically with exclusive selection and centered scrolling", async () => {
    mockSection([item(1), item(2)]);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().toggleItem("h2", 1);
    const old = useItemsStore.getState();
    let resolveTarget!: (value: unknown) => void;
    mockCommands({ resolve_library_path: () => new Promise((resolve) => { resolveTarget = resolve; }) });
    const navigation = useItemsStore.getState().revealPath("/fixture/target.jpg", () => true);
    expect(useItemsStore.getState()).toBe(old);
    mockSection(Array.from({ length: 1200 }, (_, index) => item(index + 1)));
    resolveTarget({ identity: { hash: "h901", pathId: 901 }, section: { kind: "image", month: "2026-02" } });
    expect(await navigation).toBe("revealed");
    const state = useItemsStore.getState();
    expect(state.selected).toEqual({ kind: "image", month: "2026-02" });
    expect([...state.selectedKeys]).toEqual(["h901"]);
    expect(state.selectedItem).toBe("h901");
    expect(state.items.length).toBeLessThanOrEqual(512);
    expect(state.scrollRequest).toMatchObject({ key: "h901", index: 900, align: "center" });
    expect(state.sectionMemory["image:2026-01"].anchor).toBe(old.selectedItem);
    expect(invokeCalls.some((call) => /retry|recheck/.test(call.command))).toBe(false);
  });

  it("does not reveal replacement content at an Activity target's old path", async () => {
    mockSection([item(1)]);
    await useItemsStore.getState().select(SECTION);
    const before = useItemsStore.getState();
    mockCommands({ resolve_library_path: () => ({ identity: { hash: "replacement", pathId: 1 }, section: SECTION }) });
    expect(await useItemsStore.getState().revealPath("/fixture/target.jpg", () => true, "original")).toBe("unavailable");
    expect(useItemsStore.getState()).toBe(before);
  });

  it.each(["missing", "failed", "cancelled", "new-selection", "new-sort", "disappeared"])(
    "preserves Main when diagnostic navigation is %s", async (condition) => {
      mockSection([item(1), item(2)]);
      await useItemsStore.getState().select(SECTION);
      let resolveTarget!: (value: unknown) => void;
      let rejectTarget!: (error: unknown) => void;
      mockCommands({ resolve_library_path: () => new Promise((resolve, reject) => {
        resolveTarget = resolve; rejectTarget = reject;
      }) });
      let current = true;
      const navigation = useItemsStore.getState().revealPath("/fixture/target.jpg", () => current);
      if (condition === "cancelled") current = false;
      if (condition === "new-selection") useItemsStore.getState().selectItem("h2", "nearest", 1);
      if (condition === "new-sort") {
        useItemsStore.getState().setSortOrder("name");
        await Promise.resolve();
      }
      const before = useItemsStore.getState();
      if (condition === "failed") rejectTarget(new Error("private lookup sentinel"));
      else resolveTarget(condition === "missing" ? null : {
        identity: { hash: "gone", pathId: 901 }, section: { kind: "image", month: "2026-02" },
      });
      expect(await navigation).toBe(condition === "failed" ? "failed"
        : condition === "missing" || condition === "disappeared" ? "unavailable" : "superseded");
      const after = useItemsStore.getState();
      expect(after.selected).toEqual(before.selected);
      expect(after.selectedItem).toBe(before.selectedItem);
      expect(after.selectedKeys).toEqual(before.selectedKeys);
      expect(after.scrollRequest).toEqual(before.scrollRequest);
    },
  );

  it("cancels a resolved target while its bounded section read is still pending", async () => {
    mockSection([item(1)]);
    await useItemsStore.getState().select(SECTION);
    const before = useItemsStore.getState();
    let resolveRows!: (value: SectionItem[]) => void;
    let started!: () => void;
    const reading = new Promise<void>((resolve) => { started = resolve; });
    mockCommands({ resolve_library_path: () => ({ identity: { hash: "h2", pathId: 2 }, section: SECTION }) });
    mockSection(() => new Promise((resolve) => { resolveRows = resolve; started(); }));
    let current = true;
    const navigation = useItemsStore.getState().revealPath("/fixture/target.jpg", () => current);
    await reading;
    current = false;
    resolveRows([item(2)]);
    expect(await navigation).toBe("superseded");
    expect(useItemsStore.getState()).toBe(before);
  });

  it("selects the first item and retains only the capped backend window", async () => {
    const rows = Array.from({ length: 900 }, (_, index) => item(index + 1));
    mockSection(rows);

    await useItemsStore.getState().select(SECTION);

    const state = useItemsStore.getState();
    expect(state.totalItems).toBe(900);
    expect(state.items).toHaveLength(512);
    expect(state.selectedItem).toBe("h1");
    expect(state.selectedPositions.get("h1")).toBe(0);
    expect(invokeCalls.some((call) => call.command === "reconcile_section")).toBe(true);
  });

  it("loads another capped window before selecting a remote absolute position", async () => {
    const rows = Array.from({ length: 1_200 }, (_, index) => item(index + 1));
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);

    await useItemsStore.getState().selectPosition(900, false);

    const state = useItemsStore.getState();
    expect(state.selectedItem).toBe("h901");
    expect(state.selectedPositions.get("h901")).toBe(900);
    expect(state.items.length).toBeLessThanOrEqual(512);
    expect(state.windowStart).toBeGreaterThan(0);
  });

  it("keeps the visible anchor while a refresh is in flight", async () => {
    const rows = [item(1), item(2), item(3)];
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h2", "nearest", 1);

    let release!: () => void;
    mockSection(() => new Promise<SectionItem[]>((resolve) => {
      release = () => resolve(rows);
    }));
    const pending = useItemsStore.getState().refresh();
    expect(useItemsStore.getState().selectedItem).toBe("h2");
    release();
    await pending;
    expect(useItemsStore.getState().selectedItem).toBe("h2");
  });

  it("recovers to the next remembered neighbor, then keeps it visible", async () => {
    const rows = [item(1), item(2), item(3)];
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h2", "nearest", 1);

    mockSection([item(1), item(3)]);
    await useItemsStore.getState().refresh();

    expect(useItemsStore.getState().selectedItem).toBe("h3");
    expect(useItemsStore.getState().scrollRequest).toMatchObject({ key: "h3", index: 1 });
  });

  it("does not re-centre the view when a routine refresh finds no anchor issue (R5.1 D2)", async () => {
    const rows = [item(1), item(2), item(3)];
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    // The user has since scrolled elsewhere; only the store's own scroll
    // request field distinguishes "leave the view alone" from "recentre".
    useItemsStore.setState({ scrollRequest: null });

    mockSection(rows);
    await useItemsStore.getState().refresh();

    expect(useItemsStore.getState().selectedItem).toBe("h2");
    expect(useItemsStore.getState().scrollRequest).toBeNull();
  });

  it("recovers a vanished similar-strip target near the current position, not section index 0 (R5.1 D7)", async () => {
    const rows = Array.from({ length: 10 }, (_, i) => item(i + 1));
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h5", "nearest", 4);
    expect(useItemsStore.getState().currentContext).not.toBeNull();

    // "h20" never existed in this section -- a stale similar-strip member.
    await useItemsStore.getState().selectIdentity("h20");

    // Recovery lands on a neighbor from the anchor's own recorded
    // neighborhood (h6, right after h5) rather than an empty result -- a
    // `context: null` recovery finds no neighbors and no fallback index
    // (selectFirst is false here), leaving the selection empty instead.
    expect(useItemsStore.getState().selectedItem).toBe("h6");
  });

  // R5.1 D10: a same-run return to a previously visited section restores
  // only the remembered anchor -- exclusively -- rather than whatever
  // multi-selection the section held when it was left. `sectionMemory` only
  // ever records `{ anchor, context }` (items-store.ts:250-253), so this
  // proves the actual round trip through `select()`, not just that fact.
  it("restores a same-run return's remembered anchor exclusively, dropping the prior multi-selection", async () => {
    const OTHER_SECTION = { kind: "image" as const, month: "2026-02" };
    mockSectionItems((args) =>
      (args as { month: string }).month === SECTION.month
        ? [item(1), item(2), item(3)]
        : [item(101)],
    );

    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h1", "nearest", 0);
    useItemsStore.getState().toggleItem("h2", 1);
    expect([...useItemsStore.getState().selectedKeys].sort()).toEqual(["h1", "h2"]);

    await useItemsStore.getState().select(OTHER_SECTION);
    expect(useItemsStore.getState().sectionMemory["image:2026-01"]).toMatchObject({
      anchor: "h2",
    });

    await useItemsStore.getState().select(SECTION);
    const restored = useItemsStore.getState();
    expect(restored.selectedItem).toBe("h2");
    expect([...restored.selectedKeys]).toEqual(["h2"]);
  });
});

describe("explicit selection", () => {
  it("records an anchor-only change without exposing the item identity", async () => {
    const rows = [item(1), item(2)];
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.setState({
      selectedKeys: new Set(["h1", "h2"]),
      selectedPositions: new Map([
        ["h1", 0],
        ["h2", 1],
      ]),
    });

    useItemsStore.getState().setAnchor("h2", 1);

    await vi.waitFor(() => expect(invokeCalls.some((call) => call.command === "activity_record" && (call.args.draft as { owner: string }).owner === "anchor")).toBe(true));
    const event = invokeCalls
      .filter((call) => call.command === "activity_record")
      .map((call) => call.args.draft as Record<string, unknown>)
      .find((draft) => draft.owner === "anchor");
    expect(event).toMatchObject({ kind: "changed", itemCount: 2 });
    expect(event).not.toHaveProperty("path");
    expect(event).not.toHaveProperty("hash");
  });

  it("builds and shrinks a Shift range from backend identities", async () => {
    const rows = Array.from({ length: 8 }, (_, index) => item(index + 1));
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h2", "nearest", 1);

    await useItemsStore.getState().rangeSelect("h6", 5);
    expect([...useItemsStore.getState().selectedKeys]).toEqual([
      "h2",
      "h3",
      "h4",
      "h5",
      "h6",
    ]);

    await useItemsStore.getState().rangeSelect("h4", 3);
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["h2", "h3", "h4"]);
  });

  it("does not let a delayed Shift range replace a newer ordinary selection", async () => {
    const rows = Array.from({ length: 8 }, (_, index) => item(index + 1));
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h2", "nearest", 1);

    let settleRange: ((members: Array<{ hash: string; pathId: null; index: number }>) => void) | undefined;
    mockCommands({
      get_section_range: () =>
        new Promise<Array<{ hash: string; pathId: null; index: number }>>((resolve) => {
          settleRange = resolve;
        }),
    });
    const olderRange = useItemsStore.getState().rangeSelect("h6", 5);
    useItemsStore.getState().selectItem("h8", "nearest", 7);
    settleRange?.(
      rows.slice(1, 6).map((row, offset) => ({
        hash: row.hash!,
        pathId: null,
        index: offset + 1,
      })),
    );
    await olderRange;

    expect(useItemsStore.getState().selectedItem).toBe("h8");
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["h8"]);
  });

  it("does not publish a delayed Shift-range failure after a newer selection", async () => {
    const rows = Array.from({ length: 8 }, (_, index) => item(index + 1));
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h2", "nearest", 1);

    let rejectRange: ((error: Error) => void) | undefined;
    mockCommands({
      get_section_range: () =>
        new Promise((_resolve, reject) => {
          rejectRange = reject;
        }),
    });
    const olderRange = useItemsStore.getState().rangeSelect("h6", 5);
    useItemsStore.getState().selectItem("h8", "nearest", 7);
    rejectRange?.(new Error("obsolete range failure"));
    await olderRange;

    expect(currentMainFeedback(useMainFeedbackStore.getState())).toBeNull();
    expect(
      invokeCalls.filter((call) => call.command === "record_recent_notification"),
    ).toEqual([]);
  });

  it("does not let delayed off-window keyboard navigation replace a newer click", async () => {
    const rows = Array.from({ length: 900 }, (_, index) => item(index + 1));
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);

    let settleWindow: ((window: {
      total: number;
      start: number;
      items: SectionItem[];
    }) => void) | undefined;
    mockCommands({
      get_section_window: () =>
        new Promise((resolve) => {
          settleWindow = resolve;
        }),
    });
    const olderNavigation = useItemsStore.getState().selectPosition(700, false);
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    settleWindow?.({ total: rows.length, start: 388, items: rows.slice(388) });
    await olderNavigation;

    expect(useItemsStore.getState().selectedItem).toBe("h2");
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["h2"]);
  });

  it("preserves modifier-selected keys outside a changing Shift range", async () => {
    const rows = Array.from({ length: 8 }, (_, index) => item(index + 1));
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    useItemsStore.getState().toggleItem("h8", 7);

    await useItemsStore.getState().rangeSelect("h5", 4);
    await useItemsStore.getState().rangeSelect("h3", 2);

    expect([...useItemsStore.getState().selectedKeys].sort()).toEqual([
      "h2",
      "h3",
      "h4",
      "h5",
      "h6",
      "h7",
      "h8",
    ]);
  });

  it("sends every selected identity even when the rows are outside the loaded window", async () => {
    const rows = Array.from({ length: 900 }, (_, index) => item(index + 1));
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.setState({
      selectedItem: "h700",
      selectedKeys: new Set(["h1", "h700"]),
      selectedPositions: new Map([
        ["h1", 0],
        ["h700", 699],
      ]),
    });

    await deleteItems(captureDeleteSelection(), false);

    const call = invokeCalls.find((candidate) => candidate.command === "delete_items");
    expect(call?.args.items).toEqual([
      { hash: "h1", pathId: null },
      { hash: "h700", pathId: null },
    ]);
  });
});

describe("request ownership", () => {
  it("ignores an older section response that arrives last", async () => {
    let release!: () => void;
    mockSection(() => new Promise<SectionItem[]>((resolve) => {
      release = () => resolve([item(1)]);
    }));
    const older = useItemsStore.getState().select(SECTION);

    mockSection([item(9)]);
    await useItemsStore.getState().select({ kind: "image", month: "2026-02" });
    release();
    await older;

    expect(useItemsStore.getState().selected?.month).toBe("2026-02");
    expect(useItemsStore.getState().items.map((row) => row.hash)).toEqual(["h9"]);
  });
});

// R5.1 D13: a slow detail request for a since-abandoned anchor must never
// overwrite Details once the anchor has moved on -- `loadAnchorDetail`'s
// freshness guard (items-store.ts) checks the CURRENT anchor identity when
// its response arrives, not just request order.
describe("stale detail ordering", () => {
  it("discards a detail response for an anchor Main has already left", async () => {
    mockSection([item(1), item(2)]);
    await useItemsStore.getState().select(SECTION);

    const pending = new Map<string, (detail: unknown) => void>();
    mockCommand("get_item_detail", (args) => new Promise((resolve) => {
      const hash = (args as { hash: string }).hash;
      pending.set(hash, resolve);
    }));

    useItemsStore.getState().selectItem("h1", "nearest", 0);
    useItemsStore.getState().selectItem("h2", "nearest", 1);
    expect(useItemsStore.getState().detail).toBeNull();

    // h1's request resolves last, after the anchor has already moved to h2.
    pending.get("h1")!({ fileName: "IMG_0001.jpg", kind: "image" });
    await Promise.resolve();
    await Promise.resolve();
    expect(useItemsStore.getState().detail).toBeNull();
    expect(useItemsStore.getState().selectedItem).toBe("h2");

    pending.get("h2")!({ fileName: "IMG_0002.jpg", kind: "image" });
    await vi.waitFor(() =>
      expect(useItemsStore.getState().detail?.fileName).toBe("IMG_0002.jpg"),
    );
  });
});

describe("refreshWindow", () => {
  it("reloads only the window while the section order is unchanged", async () => {
    const rows = Array.from({ length: 8 }, (_, index) => item(index + 1));
    mockSection(rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().toggleItem("h2", 1);
    useItemsStore.getState().toggleItem("h5", 4);
    const before = useItemsStore.getState();

    invokeCalls.length = 0;
    await useItemsStore.getState().refreshWindow();

    expect(invokeCalls.map((call) => call.command)).toEqual(["get_section_window"]);
    expect(useItemsStore.getState().selectedItem).toBe(before.selectedItem);
    expect(useItemsStore.getState().selectedPositions).toEqual(before.selectedPositions);
  });

  it("re-derives selection positions when items enter above the range origin", async () => {
    let rows = Array.from({ length: 8 }, (_, index) => item(index + 1));
    mockSection(() => rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h5", "nearest", 4);

    // A source check inserts twenty earlier files, so h5 now sits at 24.
    rows = [...Array.from({ length: 20 }, (_, index) => item(100 + index, { resolvedUtcMs: index })), ...rows];
    await useItemsStore.getState().refreshWindow();
    expect(invokeCalls.some((call) => call.command === "reconcile_section")).toBe(true);
    expect(useItemsStore.getState().selectedPositions.get("h5")).toBe(24);

    await useItemsStore.getState().rangeSelect("h6", 25);
    expect([...useItemsStore.getState().selectedKeys].sort()).toEqual(["h5", "h6"]);
  });

  it("drops a selected item that left the section instead of counting it", async () => {
    let rows = Array.from({ length: 4 }, (_, index) => item(index + 1));
    mockSection(() => rows);
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h3", "nearest", 2);
    useItemsStore.getState().toggleItem("h2", 1);

    // File information dates h3 into another month.
    rows = rows.filter((row) => row.hash !== "h3");
    await useItemsStore.getState().refreshWindow();
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["h2"]);
  });

  it("loads a section that refilled after it emptied", async () => {
    let rows: SectionItem[] = [];
    mockSection(() => rows);
    await useItemsStore.getState().select(SECTION);
    expect(useItemsStore.getState().totalItems).toBe(0);

    rows = [item(1), item(2)];
    await useItemsStore.getState().refreshWindow();
    expect(useItemsStore.getState().items.map((row) => row.hash)).toEqual(["h1", "h2"]);
  });

  it("serves a pending scroll load instead of reloading the region the user left", async () => {
    const rows = Array.from({ length: 1200 }, (_, index) => item(index + 1));
    let gate: Promise<void> = Promise.resolve();
    mockSection(async () => {
      await gate;
      return rows;
    });
    await useItemsStore.getState().select(SECTION);
    expect(useItemsStore.getState().windowStart).toBe(0);

    let open!: () => void;
    gate = new Promise((resolve) => {
      open = resolve;
    });
    const scroll = useItemsStore.getState().loadWindow(900);
    const refresh = useItemsStore.getState().refreshWindow();
    open();
    await Promise.all([scroll, refresh]);

    const state = useItemsStore.getState();
    expect(state.windowStart).toBe(688);
    expect(state.items[0]?.hash).toBe("h689");
  });

  it("re-derives selection positions when a scroll load supersedes it under a changed order", async () => {
    let rows = Array.from({ length: 1200 }, (_, index) => item(index + 1));
    let gate: Promise<void> = Promise.resolve();
    mockSection(async () => {
      await gate;
      return rows;
    });
    await useItemsStore.getState().select(SECTION);
    useItemsStore.getState().selectItem("h5", "nearest", 4);

    // A source check inserts twenty earlier files, so h5 now sits at 24. The
    // light refresh it triggers is superseded by a scroll load.
    rows = [...Array.from({ length: 20 }, (_, index) => item(5000 + index, { resolvedUtcMs: index })), ...rows];
    let open!: () => void;
    gate = new Promise((resolve) => {
      open = resolve;
    });
    const refresh = useItemsStore.getState().refreshWindow();
    const scroll = useItemsStore.getState().loadWindow(900);
    open();
    await Promise.all([refresh, scroll]);

    expect(useItemsStore.getState().selectedPositions.get("h5")).toBe(24);
    await useItemsStore.getState().rangeSelect("h6", 25);
    expect([...useItemsStore.getState().selectedKeys].sort()).toEqual(["h5", "h6"]);
  });

  it("does nothing when no section is selected", async () => {
    invokeCalls.length = 0;
    await useItemsStore.getState().refreshWindow();
    expect(invokeCalls).toEqual([]);
  });
});

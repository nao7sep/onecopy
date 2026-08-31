import { beforeEach, describe, expect, it } from "vitest";
import { useItemsStore } from "../../src/state/items-store";
import { deleteSelectedItems } from "../../src/workflows/items";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import {
  invokeCalls,
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
    message: null,
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
    patch_state: () => ({}),
    get_item_detail: () => ({ fileName: "item", kind: "image" }),
    get_section_counts: () => [],
    delete_items: () => ({ error: null, failedFiles: 0 }),
  });
});

describe("bounded section state", () => {
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
});

describe("explicit selection", () => {
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

    await deleteSelectedItems(false);

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

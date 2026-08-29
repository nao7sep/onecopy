// The destination panel's move actions.
//
// move-delete-rest is the app's only permanent, unrecoverable export, so the
// staging and the confirmation are destructive-safety machinery, not UI
// polish: what the dialog counts must be exactly what the confirm destroys.

import { beforeEach, describe, expect, it } from "vitest";
import { useDestinationsStore } from "../../src/state/destinations-store";
import { useItemsStore } from "../../src/state/items-store";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";
import {
  captureDestinationSelection,
  confirmDestinationDeleteRest,
  moveDestinationSelectionTo,
  moveSelectionTo,
} from "../../src/workflows/destinations";

function item(pathId: number): SectionItem {
  return {
    hash: `h${pathId}`,
    pathId,
    fileName: `IMG_${pathId}.jpg`,
    resolvedUtcMs: pathId * 1000,
    copyCount: 2,
    width: 100,
    height: 100,
    hasThumb: true,
    similarGroupId: null,
    sharpness: null,
    faceScore: null,
    byteSize: 1000,
    hasCompanions: false,
    durationMs: null,
    // copyCount is 2, so two live directories.
    dirPaths: [`/Volumes/A/photos`, `/Volumes/B/photos`],
    derivedWork: EMPTY_ITEM_WORK,
  };
}

const OUTCOME = {
  cancelled: false,
  error: null,
  items: [],
  exported: 1,
  skippedIdentical: 0,
  conflicts: [],
  undelivered: [],
  postAction: { deletedFiles: 1, failedFiles: 0, removedRows: 1 },
};

function selectAll(keys: string[]): void {
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: [item(1), item(2), item(3), item(4)],
    selectedItem: keys[0] ?? null,
    selectedKeys: new Set(keys),
    rangeOrigin: keys[0] ?? null,
    rangeBase: new Set(keys),
    sortOrders: {
      media: { order: "time", desc: false },
      other: { order: "name", desc: false },
    },
    loading: false,
    detail: null,
    message: null,
  });
}

function movedHashes(): unknown[] {
  return invokeCalls
    .filter((c) => c.command === "move_items_out")
    .flatMap((c) => c.args.items as Array<{ hash: unknown }>)
    .map((item) => item.hash);
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    move_items_out: () => OUTCOME,
    get_section_items: () => [],
    get_section_counts: () => [],
    get_issues: () => ({ total: 0, rows: [] }),
    patch_state: () => ({}),
    get_item_detail: () => null,
  });
  useDestinationsStore.setState({
    pendingDeleteRest: null,
    pendingDrop: null,
    dragSelection: null,
    message: "",
    result: null,
    confirmation: null,
    expanded: new Set(),
  });
  selectAll(["h1", "h2", "h3"]);
});

describe("loaded destination projection", () => {
  it("filters wrong-shape roots before path rendering or IPC use", () => {
    useDestinationsStore.getState().init({
      destinationRoots: [123, null, "/valid", { path: "/wrong" }],
    });

    expect(useDestinationsStore.getState().roots).toEqual(["/valid"]);
  });
});

describe("staging a permanent move", () => {
  it("asks before moving anything", async () => {
    await moveSelectionTo("/dest", "move-delete-rest");

    expect(movedHashes()).toHaveLength(0);
    const pending = useDestinationsStore.getState().pendingDeleteRest;
    expect(pending?.destDir).toBe("/dest");
    expect(pending?.count).toBe(3);
    expect(pending?.selection.items.map((entry) => entry.hash)).toEqual([
      "h1",
      "h2",
      "h3",
    ]);
  });

  it("acts on the selection it QUOTED, not the one selected later", async () => {
    await moveSelectionTo("/dest", "move-delete-rest");
    expect(useDestinationsStore.getState().pendingDeleteRest?.count).toBe(3);

    // The grid selection changes while the dialog is open — a click behind it,
    // or a watcher refresh landing on a different anchor.
    selectAll(["h4"]);
    useItemsStore.setState({ items: [item(4)] });
    await confirmDestinationDeleteRest();

    // The dialog counted three specific items. Permanently destroying a
    // different one is the failure this freeze exists to prevent.
    expect(movedHashes().sort()).toEqual(["h1", "h2", "h3"]);
  });

  it("relinquishes confirmed intent before admission so a double click submits once", async () => {
    let finish: ((outcome: typeof OUTCOME) => void) | undefined;
    mockCommands({
      move_items_out: () =>
        new Promise<typeof OUTCOME>((resolve) => {
          finish = resolve;
        }),
    });
    await moveSelectionTo("/dest", "move-delete-rest");

    const first = confirmDestinationDeleteRest();
    const second = confirmDestinationDeleteRest();

    expect(
      invokeCalls.filter((call) => call.command === "move_items_out"),
    ).toHaveLength(1);
    finish?.(OUTCOME);
    await Promise.all([first, second]);
  });

  it("cancelling moves nothing and clears the staging", () => {
    useDestinationsStore.setState({
      pendingDeleteRest: {
        destDir: "/dest",
        count: 3,
        selection: {
          items: [{ hash: "h1", pathId: null }],
          anchorKey: "h1",
          shownKeys: ["h1"],
        },
      },
    });
    useDestinationsStore.getState().cancelPendingDeleteRest();

    expect(useDestinationsStore.getState().pendingDeleteRest).toBeNull();
    expect(movedHashes()).toHaveLength(0);
  });
});

describe("the non-permanent modes", () => {
  it("move-trash-rest runs immediately over the whole selection", async () => {
    await moveSelectionTo("/dest", "move-trash-rest");

    expect(movedHashes().sort()).toEqual(["h1", "h2", "h3"]);
    expect(useDestinationsStore.getState().pendingDeleteRest).toBeNull();
  });

  it("copy leaves the originals alone", async () => {
    await moveSelectionTo("/dest", "copy");

    const modes = invokeCalls
      .filter((c) => c.command === "move_items_out")
      .map((c) => c.args.mode);
    expect(modes).toEqual(["copy"]);
  });

  it("acts on a frozen drag intent rather than a later live selection", async () => {
    selectAll(["h1"]);
    const dragged = captureDestinationSelection();
    selectAll(["h4"]);

    await moveDestinationSelectionTo("/dest", "copy", dragged);

    expect(movedHashes()).toEqual(["h1"]);
  });
});

describe("outcome reporting", () => {
  it("spells out a conflict instead of swallowing it", async () => {
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        exported: 0,
        conflicts: ["/dest/IMG_1.jpg"],
        postAction: { deletedFiles: 0, failedFiles: 0, removedRows: 0 },
      }),
    });
    selectAll(["h1"]);

    await moveSelectionTo("/dest", "copy");

    expect(useDestinationsStore.getState().result?.severity).toBe("warning");
    expect(useDestinationsStore.getState().result?.message).toMatch(/different content/);
    expect(useDestinationsStore.getState().result?.message).toContain("IMG_1.jpg");
  });

  it("reports a target nothing could be written to", async () => {
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        exported: 0,
        undelivered: ["/dest/IMG_1.arw"],
        postAction: { deletedFiles: 0, failedFiles: 0, removedRows: 0 },
      }),
    });
    selectAll(["h1"]);

    await moveSelectionTo("/dest", "copy");

    expect(useDestinationsStore.getState().result?.severity).toBe("error");
    expect(useDestinationsStore.getState().result?.message).toMatch(/Could not write/);
    expect(useDestinationsStore.getState().result?.message).toContain("IMG_1.arw");
  });

  it("keeps source post-action failures visible after progress closes", async () => {
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        postAction: { deletedFiles: 1, failedFiles: 2, removedRows: 1 },
      }),
    });
    selectAll(["h1"]);

    await moveSelectionTo("/dest", "move-trash-rest");

    expect(useDestinationsStore.getState().result?.severity).toBe("error");
    expect(useDestinationsStore.getState().result?.message).toContain(
      "2 originals could not be handled",
    );
    expect(useDestinationsStore.getState().result?.message).toContain("Issues");
  });

  it("says so plainly when nothing is selected", async () => {
    useItemsStore.setState({ selectedKeys: new Set(), selectedItem: null });

    await moveSelectionTo("/dest", "copy");

    expect(movedHashes()).toHaveLength(0);
    expect(useDestinationsStore.getState().result?.severity).toBe("warning");
    expect(useDestinationsStore.getState().result?.message).toMatch(/select an item/i);
  });

  it("keeps an unresolved result through an unrelated full success", async () => {
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        exported: 0,
        conflicts: ["/old/IMG_1.jpg"],
        postAction: { deletedFiles: 0, failedFiles: 0, removedRows: 0 },
      }),
    });
    selectAll(["h1"]);
    await moveSelectionTo("/old", "copy");
    const unresolved = useDestinationsStore.getState().result;

    mockCommands({ move_items_out: () => OUTCOME });
    selectAll(["h1"]);
    await moveSelectionTo("/dest", "copy");

    expect(useDestinationsStore.getState().result).toEqual(unresolved);
    expect(useDestinationsStore.getState().confirmation).toBe(
      "Copied 1 file to dest.",
    );
  });

  it("clears a result when the same operation succeeds on retry", async () => {
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        exported: 0,
        conflicts: ["/dest/IMG_1.jpg"],
        postAction: { deletedFiles: 0, failedFiles: 0, removedRows: 0 },
      }),
    });
    selectAll(["h1"]);
    await moveSelectionTo("/dest", "copy");
    expect(useDestinationsStore.getState().result).not.toBeNull();

    mockCommands({ move_items_out: () => OUTCOME });
    selectAll(["h1"]);
    await moveSelectionTo("/dest", "copy");

    expect(useDestinationsStore.getState().result).toBeNull();
    expect(useDestinationsStore.getState().confirmation).toBe(
      "Copied 1 file to dest.",
    );
  });

  it("clears the selection-required result after the requested action succeeds", async () => {
    useItemsStore.setState({ selectedKeys: new Set(), selectedItem: null });
    await moveSelectionTo("/dest", "copy");
    expect(useDestinationsStore.getState().result?.message).toMatch(/select an item/i);

    selectAll(["h1"]);
    await moveSelectionTo("/dest", "copy");

    expect(useDestinationsStore.getState().result).toBeNull();
  });

  it("accounts for completed work in one partial result", async () => {
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        exported: 2,
        conflicts: ["/dest/IMG_3.jpg"],
        postAction: { deletedFiles: 1, failedFiles: 0, removedRows: 1 },
      }),
    });
    selectAll(["h1", "h2", "h3"]);

    await moveSelectionTo("/dest", "move-trash-rest");

    const result = useDestinationsStore.getState().result;
    expect(result?.severity).toBe("warning");
    expect(result?.message).toContain("2 files delivered");
    expect(result?.message).toContain("1 original handled");
    expect(result?.message).toContain("IMG_3.jpg");
  });

  it("recovers selection beside a source row removed by Move", async () => {
    selectAll(["h1"]);
    mockCommands({
      move_items_out: () => OUTCOME,
      get_section_items: () => [item(2), item(3), item(4)],
    });

    await moveSelectionTo("/dest", "move-trash-rest");

    expect(useItemsStore.getState().selectedItem).toBe("h2");
  });

  it("keeps surviving multi-selection members when its anchor was moved", async () => {
    selectAll(["h1", "h2", "h3"]);
    useItemsStore.setState({ selectedItem: "h2" });
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        conflicts: ["/dest/IMG_1.jpg", "/dest/IMG_3.jpg"],
      }),
      get_section_items: () => [item(1), item(3), item(4)],
    });

    await moveSelectionTo("/dest", "move-trash-rest");

    const state = useItemsStore.getState();
    expect(state.selectedItem).toBe("h3");
    expect(state.selectedKeys).toEqual(new Set(["h1", "h3"]));
  });
});

describe("a folder created inside the app is immediately usable", () => {
  it("expands the parent and actives the new folder", async () => {
    // The developer's report: "even if we make a subfolder, it won't be shown
    // like a tree". The folder existed on disk; the tree could not show it.
    mockCommands({
      create_subdir: () => "/dest/photos/2026",
      list_subdirs: () => [
        {
          name: "2026",
          path: "/dest/photos/2026",
          hasChildren: false,
          isEmpty: true,
        },
      ],
    });
    useDestinationsStore.setState({
      roots: ["/dest/photos"],
      children: {},
      expanded: new Set(),
      activePath: "/dest/photos",
    });

    await useDestinationsStore.getState().createFolder("/dest/photos", "2026");

    const state = useDestinationsStore.getState();
    expect(state.children["/dest/photos"]?.map((c) => c.name)).toEqual([
      "2026",
    ]);
    expect(state.expanded.has("/dest/photos")).toBe(true);
    expect(state.activePath).toBe("/dest/photos/2026");
  });
});

describe("expandability reads the live children map", () => {
  it("a leaf that gained a child becomes expandable without re-listing its parent", async () => {
    const { nodeHasChildren } =
      await import("../../src/components/DestinationsTab");
    // The grandparent's listing said "no children" when it was true...
    const entry = { path: "/dest/photos/2026", hasChildren: false };
    // ...but the node has since been listed itself and HAS one.
    const children = {
      "/dest/photos/2026": [
        {
          name: "spain",
          path: "/dest/photos/2026/spain",
          hasChildren: false,
          isEmpty: true,
        },
      ],
    };
    expect(nodeHasChildren(entry, children)).toBe(true);
    // Unlisted nodes still trust the snapshot, in both directions.
    expect(nodeHasChildren({ path: "/x", hasChildren: true }, {})).toBe(true);
    expect(nodeHasChildren({ path: "/x", hasChildren: false }, {})).toBe(false);
  });
});

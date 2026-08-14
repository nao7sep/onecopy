// The destination panel's move actions.
//
// move-delete-rest is the app's only permanent, unrecoverable export, so the
// staging and the confirmation are destructive-safety machinery, not UI
// polish: what the dialog counts must be exactly what the confirm destroys.

import { beforeEach, describe, expect, it } from "vitest";
import { useDestinationsStore } from "../../src/state/destinations-store";
import { useItemsStore } from "../../src/state/items-store";
import type { SectionItem } from "../../src/models/items";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

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
    byteSize: 1000,
    hasCompanions: false,
    durationMs: null,
  };
}

const OUTCOME = {
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
    sortOrder: "time",
    loading: false,
    detail: null,
    message: null,
  });
}

function movedHashes(): unknown[] {
  return invokeCalls
    .filter((c) => c.command === "move_item_out")
    .map((c) => c.args.hash);
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    move_item_out: () => OUTCOME,
    get_section_items: () => [],
    get_section_counts: () => [],
    get_issues: () => ({ total: 0, rows: [] }),
    patch_state: () => ({}),
    get_item_detail: () => null,
  });
  useDestinationsStore.setState({ pendingDeleteRest: null, message: "" });
  selectAll(["h1", "h2", "h3"]);
});

describe("staging a permanent move", () => {
  it("asks before moving anything", async () => {
    await useDestinationsStore
      .getState()
      .moveSelectionTo("/dest", "move-delete-rest");

    expect(movedHashes()).toHaveLength(0);
    const pending = useDestinationsStore.getState().pendingDeleteRest;
    expect(pending?.destDir).toBe("/dest");
    expect(pending?.count).toBe(3);
    expect(pending?.confirmed).toBe(false);
  });

  it("acts on the selection it QUOTED, not the one selected later", async () => {
    await useDestinationsStore
      .getState()
      .moveSelectionTo("/dest", "move-delete-rest");
    expect(useDestinationsStore.getState().pendingDeleteRest?.count).toBe(3);

    // The grid selection changes while the dialog is open — a click behind it,
    // or a watcher refresh landing on a different anchor.
    selectAll(["h4"]);
    await useDestinationsStore.getState().confirmPendingDeleteRest();

    // The dialog counted three specific items. Permanently destroying a
    // different one is the failure this freeze exists to prevent.
    expect(movedHashes().sort()).toEqual(["h1", "h2", "h3"]);
  });

  it("cancelling moves nothing and clears the staging", () => {
    useDestinationsStore.setState({
      pendingDeleteRest: { destDir: "/dest", count: 3, confirmed: false, keys: ["h1"] },
    });
    useDestinationsStore.getState().cancelPendingDeleteRest();

    expect(useDestinationsStore.getState().pendingDeleteRest).toBeNull();
    expect(movedHashes()).toHaveLength(0);
  });
});

describe("the non-permanent modes", () => {
  it("move-trash-rest runs immediately over the whole selection", async () => {
    await useDestinationsStore
      .getState()
      .moveSelectionTo("/dest", "move-trash-rest");

    expect(movedHashes().sort()).toEqual(["h1", "h2", "h3"]);
    expect(useDestinationsStore.getState().pendingDeleteRest).toBeNull();
  });

  it("copy leaves the originals alone", async () => {
    await useDestinationsStore.getState().moveSelectionTo("/dest", "copy");

    const modes = invokeCalls
      .filter((c) => c.command === "move_item_out")
      .map((c) => c.args.mode);
    expect(modes).toEqual(["copy", "copy", "copy"]);
  });
});

describe("outcome reporting", () => {
  it("spells out a conflict instead of swallowing it", async () => {
    mockCommands({
      move_item_out: () => ({
        ...OUTCOME,
        exported: 0,
        conflicts: ["/dest/IMG_1.jpg"],
        postAction: { deletedFiles: 0, failedFiles: 0, removedRows: 0 },
      }),
    });
    selectAll(["h1"]);

    await useDestinationsStore.getState().moveSelectionTo("/dest", "copy");

    expect(useDestinationsStore.getState().message).toMatch(/CONFLICT/);
    expect(useDestinationsStore.getState().message).toContain("IMG_1.jpg");
  });

  it("reports a target nothing could be written to", async () => {
    mockCommands({
      move_item_out: () => ({
        ...OUTCOME,
        exported: 0,
        undelivered: ["/dest/IMG_1.arw"],
        postAction: { deletedFiles: 0, failedFiles: 0, removedRows: 0 },
      }),
    });
    selectAll(["h1"]);

    await useDestinationsStore.getState().moveSelectionTo("/dest", "copy");

    expect(useDestinationsStore.getState().message).toMatch(/FAILED/);
    expect(useDestinationsStore.getState().message).toContain("IMG_1.arw");
  });

  it("says so plainly when nothing is selected", async () => {
    useItemsStore.setState({ selectedKeys: new Set(), selectedItem: null });

    await useDestinationsStore.getState().moveSelectionTo("/dest", "copy");

    expect(movedHashes()).toHaveLength(0);
    expect(useDestinationsStore.getState().message).toMatch(/select an item/i);
  });
});

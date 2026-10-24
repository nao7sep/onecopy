// The destination panel's move actions.
//
// Both Move modes clean up source copies, so staging and confirmation are
// destructive-safety machinery, not UI polish: what the dialog counts must
// be exactly what the confirm changes.

import { EMPTY_ITEM_WORK } from "../helpers/itemWork";
import { beforeEach, describe, expect, it } from "vitest";
import { useDestinationsStore, type DirListing } from "../../src/state/destinations-store";
import { useItemsStore } from "../../src/state/items-store";
import type { SectionItem } from "../../src/models/items";
import {
  invokeCalls,
  mockCommands,
  mockSectionItems,
  resetTauriMocks,
} from "../mocks/tauri";
import {
  captureDestinationSelection,
  confirmDestinationMove,
  moveDestinationSelectionTo,
  moveSelectionTo,
  resolveDestinationConflicts,
} from "../../src/workflows/destinations";
import { english, inEnglish } from "../helpers/i18n";

// The sentence the destination panel shows: its facts joined and terminated the
// way DestinationsTab renders them, so a test still asserts what a person reads.
function resultLine(): string | null {
  const result = useDestinationsStore.getState().result;
  return result === null
    ? null
    : english.t("destinations.resultLine", {
        facts: result.facts.map((fact) => english.text(fact)).join(" · "),
      });
}

function confirmationLine(): string | null {
  return inEnglish(useDestinationsStore.getState().confirmation);
}

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

it("invalidates old visibility listings and removes a now-hidden destination", async () => {
  const entry = (path: string) => ({ path, name: path.split("/").at(-1)!, hasChildren: false, isEmpty: false });
  useDestinationsStore.setState({ roots: ["/dest"], activePath: "/dest/.hidden", expanded: new Set(["/dest", "/dest/.hidden"]),
    children: { "/dest": [entry("/dest/.hidden"), entry("/dest/visible")] }, listing: {}, emptiness: {} });
  let oldResolve!: (listing: DirListing) => void;
  mockCommands({ list_subdirs: () => new Promise<DirListing>((resolve) => { oldResolve = resolve; }) });
  const stale = useDestinationsStore.getState().refreshNode("/dest");
  mockCommands({ list_subdirs: () => ({ entries: [entry("/dest/visible")], isEmpty: false }) });
  await useDestinationsStore.getState().reconcileVisibility();
  oldResolve({ entries: [entry("/dest/.hidden")], isEmpty: true });
  await stale;
  expect(useDestinationsStore.getState().activePath).toBeNull();
  expect([...useDestinationsStore.getState().expanded]).toEqual(["/dest"]);
  expect(useDestinationsStore.getState().children["/dest"]).toEqual([entry("/dest/visible")]);
});

const OUTCOME = {
  cancelled: false,
  error: null,
  items: [],
  exported: 1,
  skippedIdentical: 0,
  conflicts: [],
  undelivered: [],
  differentCompanions: [],
  postAction: { deletedFiles: 1, failedFiles: 0, removedRows: 1 },
  planToken: null,
  requiresConflictChoice: false,
  planChanged: false,
  overwriteAllowed: true,
  reviewedConflicts: [],
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
    get_section_counts: () => [],
    get_issues: () => ({ total: 0, rows: [] }),
    patch_state: () => ({}),
    get_item_detail: () => null,
  });
  mockSectionItems(() => []);
  useDestinationsStore.setState({
    pendingMove: null,
    pendingDrop: null,
    pendingConflicts: null,
    dragSelection: null,
    message: null,
    result: null,
    confirmation: null,
    expanded: new Set(),
    roots: [],
    children: {},
    emptiness: {},
    listing: {},
    activePath: null,
  });
  selectAll(["h1", "h2", "h3"]);
});

describe("loaded destination projection", () => {
  it("retires the last root and stops queued refreshes after the held listing settles", async () => {
    useDestinationsStore.getState().init({ destinationRoots: ["/dest"] });
    const child = { path: "/dest/child", name: "child", hasChildren: false, isEmpty: true };
    useDestinationsStore.setState({ activePath: child.path, expanded: new Set(["/dest", child.path]),
      children: { "/dest": [child], [child.path]: [] }, emptiness: { "/dest": false, [child.path]: true } });
    let finish!: (listing: DirListing) => void;
    mockCommands({ list_subdirs: () => new Promise<DirListing>((resolve) => { finish = resolve; }) });
    const refresh = useDestinationsStore.getState().refreshExpanded();
    useDestinationsStore.getState().init({ destinationRoots: [] });
    expect(useDestinationsStore.getState()).toMatchObject({ roots: [], activePath: null,
      children: {}, emptiness: {}, listing: {}, expanded: new Set() });
    finish({ entries: [child], isEmpty: false });
    await refresh;
    await useDestinationsStore.getState().refreshExpanded();
    expect(invokeCalls.filter((call) => call.command === "list_subdirs").map((call) => call.args.path)).toEqual(["/dest"]);
    expect(useDestinationsStore.getState()).toMatchObject({ activePath: null, children: {}, emptiness: {}, listing: {} });
  });

  it("preserves a remaining nested root and its loaded descendants without retaining sibling branches", () => {
    const entry = (path: string) => ({ path, name: path.split("/").at(-1)!, hasChildren: true, isEmpty: false });
    useDestinationsStore.getState().init({ destinationRoots: ["/dest", "/dest/keep"] });
    const children = { "/dest": [entry("/dest/keep"), entry("/dest/keep-sibling")],
      "/dest/keep": [entry("/dest/keep/child")], "/dest/keep/child": [], "/dest/keep-sibling": [] };
    useDestinationsStore.setState({ children, expanded: new Set(Object.keys(children)),
      emptiness: Object.fromEntries(Object.keys(children).map((path) => [path, false])), activePath: "/dest/keep/child" });
    useDestinationsStore.getState().init({ destinationRoots: ["/dest/keep"] });
    expect(Object.keys(useDestinationsStore.getState().children)).toEqual(["/dest/keep", "/dest/keep/child"]);
    expect([...useDestinationsStore.getState().expanded]).toEqual(["/dest/keep", "/dest/keep/child"]);
    expect(Object.keys(useDestinationsStore.getState().emptiness)).toEqual(["/dest/keep", "/dest/keep/child"]);
    expect(useDestinationsStore.getState().activePath).toBe("/dest/keep/child");
    useDestinationsStore.getState().init({ destinationRoots: ["/dest/keep-sibling"] });
    expect(useDestinationsStore.getState().activePath).toBeNull();
  });

  it("filters wrong-shape roots before path rendering or IPC use", () => {
    useDestinationsStore.getState().init({
      destinationRoots: [123, null, "/valid", { path: "/wrong" }],
    });

    expect(useDestinationsStore.getState().roots).toEqual(["/valid"]);
  });
});

describe("destination listing freshness", () => {
  it("does not let an older folder response overwrite newer children or own emptiness", async () => {
    let finishOlder!: (listing: DirListing) => void;
    mockCommands({ list_subdirs: () => new Promise<DirListing>((resolve) => { finishOlder = resolve; }) });
    const older = useDestinationsStore.getState().refreshNode("/dest");
    const entries = [{ name: "new", path: "/dest/new", hasChildren: false, isEmpty: true }];
    mockCommands({ list_subdirs: () => ({ entries, isEmpty: false }) });
    await useDestinationsStore.getState().refreshNode("/dest");
    finishOlder({ entries: [], isEmpty: true });
    await older;

    expect(useDestinationsStore.getState().children["/dest"]).toEqual(entries);
    expect(useDestinationsStore.getState().emptiness).toMatchObject({ "/dest": false, "/dest/new": true });
  });

  it("invalidates an in-flight listing when its root is removed", async () => {
    useDestinationsStore.getState().init({ destinationRoots: ["/dest"] });
    let finish!: (listing: DirListing) => void;
    mockCommands({ list_subdirs: () => new Promise<DirListing>((resolve) => { finish = resolve; }) });
    const pending = useDestinationsStore.getState().refreshNode("/dest");
    useDestinationsStore.getState().init({ destinationRoots: ["/other"] });
    finish({ entries: [], isEmpty: true });
    await pending;
    expect(useDestinationsStore.getState().children["/dest"]).toBeUndefined();
    expect(useDestinationsStore.getState().emptiness["/dest"]).toBeUndefined();
    expect(useDestinationsStore.getState().listing["/dest"]).toBeUndefined();
  });
});

describe("staging a destructive move", () => {
  it("asks before moving anything", async () => {
    await moveSelectionTo("/dest", "move-delete-rest");

    expect(movedHashes()).toHaveLength(0);
    const pending = useDestinationsStore.getState().pendingMove;
    expect(pending?.destDir).toBe("/dest");
    expect(pending?.count).toBe(3);
    expect(pending?.mode).toBe("move-delete-rest");
    expect(pending?.selection.items.map((entry) => entry.hash)).toEqual([
      "h1",
      "h2",
      "h3",
    ]);
  });

  it("acts on the selection it QUOTED, not the one selected later", async () => {
    await moveSelectionTo("/dest", "move-delete-rest");
    expect(useDestinationsStore.getState().pendingMove?.count).toBe(3);

    // The grid selection changes while the dialog is open — a click behind it,
    // or a watcher refresh landing on a different anchor.
    selectAll(["h4"]);
    useItemsStore.setState({ items: [item(4)] });
    await confirmDestinationMove();

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

    const first = confirmDestinationMove();
    const second = confirmDestinationMove();

    expect(
      invokeCalls.filter((call) => call.command === "move_items_out"),
    ).toHaveLength(1);
    finish?.(OUTCOME);
    await Promise.all([first, second]);
  });

  it("cancelling moves nothing and clears the staging", () => {
    useDestinationsStore.setState({
      pendingMove: {
        destDir: "/dest",
        count: 3,
        mode: "move-delete-rest",
        selection: {
          items: [{ hash: "h1", pathId: null }],
          anchorKey: "h1",
        },
      },
    });
    useDestinationsStore.getState().cancelPendingMove();

    expect(useDestinationsStore.getState().pendingMove).toBeNull();
    expect(movedHashes()).toHaveLength(0);
  });
});

describe("the non-permanent modes", () => {
  it("move-trash-rest reviews the whole frozen selection before acting", async () => {
    await moveSelectionTo("/dest", "move-trash-rest");

    expect(movedHashes()).toHaveLength(0);
    expect(useDestinationsStore.getState().pendingMove).toMatchObject({
      destDir: "/dest",
      count: 3,
      mode: "move-trash-rest",
    });
    await confirmDestinationMove();
    expect(movedHashes().sort()).toEqual(["h1", "h2", "h3"]);
    expect(useDestinationsStore.getState().pendingMove).toBeNull();
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

describe("destination conflict review", () => {
  it("does nothing until one policy is accepted for the frozen selection", async () => {
    let attempts = 0;
    mockCommands({
      move_items_out: () => {
        attempts += 1;
        if (attempts === 1) {
          return {
            ...OUTCOME,
            exported: 0,
            postAction: { deletedFiles: 0, failedFiles: 0, removedRows: 0 },
            planToken: "reviewed-plan",
            requiresConflictChoice: true,
            reviewedConflicts: [
              {
                path: "/dest/IMG_1.jpg",
                incomingBytes: 1000,
                existingBytes: 900,
                withinSelection: false,
                preservedPaths: [
                  "/dest/IMG_1.jpg",
                  "/dest/IMG_1.xmp",
                ],
              },
            ],
          };
        }
        return OUTCOME;
      },
    });
    selectAll(["h1"]);

    await moveSelectionTo("/dest", "copy");

    const pending = useDestinationsStore.getState().pendingConflicts;
    expect(pending?.planToken).toBe("reviewed-plan");
    expect(pending?.selection.items.map((entry) => entry.hash)).toEqual(["h1"]);
    expect(useDestinationsStore.getState().result).toBeNull();
    expect(movedHashes()).toEqual(["h1"]);

    selectAll(["h4"]);
    await resolveDestinationConflicts("rename");

    const calls = invokeCalls.filter((call) => call.command === "move_items_out");
    expect(calls).toHaveLength(2);
    expect(calls[1]?.args.items).toEqual([{ hash: "h1", pathId: null }]);
    expect(calls[1]?.args.conflictPolicy).toBe("rename");
    expect(calls[1]?.args.planToken).toBe("reviewed-plan");
    expect(useDestinationsStore.getState().pendingConflicts).toBeNull();
  });

  it("refuses a changed review instead of broadening it", async () => {
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        exported: 0,
        planChanged: true,
        planToken: "new-plan",
        requiresConflictChoice: true,
        reviewedConflicts: [
          {
            path: "/dest/IMG_1.jpg",
            incomingBytes: 1000,
            existingBytes: 900,
            withinSelection: false,
            preservedPaths: ["/dest/IMG_1.jpg"],
          },
        ],
        postAction: { deletedFiles: 0, failedFiles: 0, removedRows: 0 },
      }),
    });
    useDestinationsStore.setState({
      pendingConflicts: {
        destDir: "/dest",
        mode: "copy",
        selection: captureDestinationSelection(),
        planToken: "old-plan",
        conflicts: [],
        overwriteAllowed: true,
      },
    });

    await resolveDestinationConflicts("overwrite");

    expect(useDestinationsStore.getState().result?.severity).toBe("warning");
    expect(resultLine()).toMatch(/changed.*review/i);
  });
});

describe("outcome reporting", () => {
  it("spells out a new unreviewed conflict instead of swallowing it", async () => {
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
    expect(resultLine()).toMatch(/different content/);
    expect(resultLine()).toContain("IMG_1.jpg");
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
    expect(resultLine()).toMatch(/Could not write/);
    expect(resultLine()).toContain("IMG_1.arw");
  });

  it("says which differing companions a move left in place", async () => {
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        differentCompanions: ["/src/b/IMG_1.xmp"],
      }),
    });
    selectAll(["h1"]);

    await moveSelectionTo("/dest", "move-trash-rest");
    await confirmDestinationMove();

    expect(useDestinationsStore.getState().result?.severity).toBe("warning");
    expect(resultLine()).toMatch(/Left in place with the copy beside them/);
    expect(resultLine()).toContain("/src/b/IMG_1.xmp");
  });

  it("says which differing companions a copy did not deliver", async () => {
    mockCommands({
      move_items_out: () => ({
        ...OUTCOME,
        postAction: { deletedFiles: 0, failedFiles: 0, removedRows: 0 },
        differentCompanions: ["/src/b/IMG_1.xmp"],
      }),
    });
    selectAll(["h1"]);

    await moveSelectionTo("/dest", "copy");

    expect(useDestinationsStore.getState().result?.severity).toBe("warning");
    expect(resultLine()).toMatch(/Not copied because they differ/);
    expect(resultLine()).toContain("/src/b/IMG_1.xmp");
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
    await confirmDestinationMove();

    expect(useDestinationsStore.getState().result?.severity).toBe("error");
    expect(resultLine()).toContain("2 originals could not be handled");
    expect(resultLine()).toContain("Issues");
  });

  it("says so plainly when nothing is selected", async () => {
    useItemsStore.setState({ selectedKeys: new Set(), selectedItem: null });

    await moveSelectionTo("/dest", "copy");

    expect(movedHashes()).toHaveLength(0);
    expect(useDestinationsStore.getState().result?.severity).toBe("warning");
    expect(resultLine()).toMatch(/select an item/i);
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
    expect(confirmationLine()).toBe("Copied 1 file to dest.");
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
    expect(confirmationLine()).toBe("Copied 1 file to dest.");
  });

  it("clears the selection-required result after the requested action succeeds", async () => {
    useItemsStore.setState({ selectedKeys: new Set(), selectedItem: null });
    await moveSelectionTo("/dest", "copy");
    expect(resultLine()).toMatch(/select an item/i);

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
    await confirmDestinationMove();

    const result = useDestinationsStore.getState().result;
    expect(result?.severity).toBe("warning");
    expect(resultLine()).toContain("2 files delivered");
    expect(resultLine()).toContain("1 original handled");
    expect(resultLine()).toContain("IMG_3.jpg");
  });

  it("recovers selection beside a source row removed by Move", async () => {
    selectAll(["h1"]);
    mockCommands({ move_items_out: () => OUTCOME });
    mockSectionItems(() => [item(2), item(3), item(4)]);

    await moveSelectionTo("/dest", "move-trash-rest");
    await confirmDestinationMove();

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
    });
    mockSectionItems(() => [item(1), item(3), item(4)]);

    await moveSelectionTo("/dest", "move-trash-rest");
    await confirmDestinationMove();

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
      list_subdirs: () => ({ entries: [
        {
          name: "2026",
          path: "/dest/photos/2026",
          hasChildren: false,
          isEmpty: true,
        },
      ], isEmpty: false }),
    });
    useDestinationsStore.setState({
      roots: ["/dest/photos"],
      children: {},
      expanded: new Set(),
      activePath: "/dest/photos",
      emptiness: { "/dest/photos": true },
    });

    await useDestinationsStore.getState().createFolder("/dest/photos", "2026");

    const state = useDestinationsStore.getState();
    expect(state.children["/dest/photos"]?.map((c) => c.name)).toEqual([
      "2026",
    ]);
    expect(state.expanded.has("/dest/photos")).toBe(true);
    expect(state.activePath).toBe("/dest/photos/2026");
    expect(state.emptiness["/dest/photos"]).toBe(false);
    expect(state.emptiness["/dest/photos/2026"]).toBe(true);
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

describe("destination folder mutations", () => {
  it.each([true, false])("refreshes the parent's actual emptiness after deleting its last child: %s", async (isEmpty) => {
    mockCommands({ delete_empty_dir: () => null, list_subdirs: () => ({ entries: [], isEmpty }) });
    useDestinationsStore.setState({
      activePath: "/dest/child",
      expanded: new Set(["/dest", "/dest/child", "/dest/child/nested", "/dest/childish"]),
      children: { "/dest/child": [], "/dest/child/nested": [], "/dest/childish": [] },
      emptiness: { "/dest": false, "/dest/child": true, "/dest/child/nested": true, "/dest/childish": true },
    });
    await useDestinationsStore.getState().deleteFolder("/dest/child", "/dest");
    const state = useDestinationsStore.getState();
    expect(state.activePath).toBe("/dest");
    expect(state.emptiness).toEqual({ "/dest": isEmpty, "/dest/childish": true });
    expect(state.children).toEqual({ "/dest": [], "/dest/childish": [] });
    expect([...state.expanded]).toEqual(["/dest", "/dest/childish"]);
  });

  it.each(["create", "delete"])("preserves a selection changed during %s", async (operation) => {
    let finish!: (value: string | null) => void;
    mockCommands({
      create_subdir: () => new Promise((resolve) => { finish = resolve; }),
      delete_empty_dir: () => new Promise((resolve) => { finish = resolve; }),
      list_subdirs: () => ({ entries: [], isEmpty: true }),
    });
    useDestinationsStore.getState().setActive("/dest/child");
    const pending = operation === "create"
      ? useDestinationsStore.getState().createFolder("/dest", "new")
      : useDestinationsStore.getState().deleteFolder("/dest/child", "/dest");
    useDestinationsStore.getState().setActive("/other");
    finish(operation === "create" ? "/dest/new" : null);
    await pending;
    expect(useDestinationsStore.getState().activePath).toBe("/other");
  });

  it.each(["create", "delete"])("preserves a selection changed during the parent refresh after %s", async (operation) => {
    let finish!: (listing: DirListing) => void;
    let started!: () => void;
    const refreshing = new Promise<void>((resolve) => { started = resolve; });
    mockCommands({
      create_subdir: () => "/dest/new",
      delete_empty_dir: () => null,
      list_subdirs: () => {
        started();
        return new Promise<DirListing>((resolve) => { finish = resolve; });
      },
    });
    useDestinationsStore.getState().setActive("/dest/child");
    const pending = operation === "create"
      ? useDestinationsStore.getState().createFolder("/dest", "new")
      : useDestinationsStore.getState().deleteFolder("/dest/child", "/dest");
    await refreshing;
    useDestinationsStore.getState().setActive("/other");
    finish({ entries: [], isEmpty: true });
    await pending;
    expect(useDestinationsStore.getState().activePath).toBe("/other");
  });

  it("does not resurrect a deleted subtree from a pending listing", async () => {
    let finish!: (listing: DirListing) => void;
    mockCommands({ list_subdirs: () => new Promise<DirListing>((resolve) => { finish = resolve; }) });
    const listing = useDestinationsStore.getState().refreshNode("/dest/child");
    mockCommands({ delete_empty_dir: () => null, list_subdirs: () => ({ entries: [], isEmpty: true }) });
    await useDestinationsStore.getState().deleteFolder("/dest/child", "/dest");
    finish({ entries: [], isEmpty: true });
    await listing;
    expect(useDestinationsStore.getState().children["/dest/child"]).toBeUndefined();
    expect(useDestinationsStore.getState().emptiness["/dest/child"]).toBeUndefined();
  });

  it("does not activate a created folder after the roots change", async () => {
    let finish!: (path: string) => void;
    mockCommands({ create_subdir: () => new Promise<string>((resolve) => { finish = resolve; }) });
    useDestinationsStore.getState().init({ destinationRoots: ["/dest"] });
    useDestinationsStore.getState().setActive("/dest");
    const pending = useDestinationsStore.getState().createFolder("/dest", "new");
    useDestinationsStore.getState().init({ destinationRoots: ["/other"] });
    finish("/dest/new");
    await pending;
    expect(useDestinationsStore.getState().activePath).toBeNull();
    expect(invokeCalls.some((call) => call.command === "list_subdirs")).toBe(false);
  });
});

it("does not replace a newly refreshed node fact with an older parent's child fact", async () => {
  let finish!: (listing: DirListing) => void;
  mockCommands({ list_subdirs: () => new Promise<DirListing>((resolve) => { finish = resolve; }) });
  const parent = useDestinationsStore.getState().refreshNode("/dest");
  mockCommands({ list_subdirs: () => ({ entries: [], isEmpty: false }) });
  await useDestinationsStore.getState().refreshNode("/dest/child");
  finish({ entries: [{ path: "/dest/child", name: "child", hasChildren: false, isEmpty: true }], isEmpty: false });
  await parent;
  expect(useDestinationsStore.getState().emptiness["/dest/child"]).toBe(false);
});

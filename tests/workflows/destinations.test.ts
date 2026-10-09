// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import {
  addDestinationRoot,
  beginDestinationDrag,
  removeDestinationRoot,
} from "../../src/workflows/destinations";
import { useDestinationsStore } from "../../src/state/destinations-store";
import { resetConfigWritesForTests, useAppStore } from "../../src/state/app-store";
import { useItemsStore } from "../../src/state/items-store";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import {
  invokeCalls,
  mockCommands,
  openDialog,
  resetTauriMocks,
} from "../mocks/tauri";
import { inEnglish } from "../helpers/i18n";

function item(pathId: number): SectionItem {
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
    sharpness: null,
    faceScore: null,
    byteSize: 1000,
    hasCompanions: false,
    durationMs: null,
    dirPaths: ["/photos"],
    derivedWork: EMPTY_ITEM_WORK,
  };
}

// The core replies with the whole effective config after each save.
function savedConfig(changes: unknown): Record<string, unknown> {
  return { ...useAppStore.getState().appData!.config, ...(changes as object) };
}

async function settle(): Promise<void> {
  for (let index = 0; index < 20; index += 1) await Promise.resolve();
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  resetConfigWritesForTests();
  mockCommands({ save_config: ({ changes }) => savedConfig(changes) });
  useAppStore.setState({
    appData: {
      config: { destinationRoots: ["/existing"] },
      state: {},
      dataRoot: "/app",
      debugEnabled: false,
      quarantines: [],
    },
  });
  useDestinationsStore.setState({ roots: ["/existing"], message: null });
});

describe("destination root failures", () => {
  it("keeps a failed directory picker visible", async () => {
    openDialog.mockRejectedValueOnce(new Error("picker unavailable"));

    await addDestinationRoot();

    expect(useDestinationsStore.getState().roots).toEqual(["/existing"]);
    expect(inEnglish(useDestinationsStore.getState().message)).toBe(
      "Couldn’t add that destination.",
    );
  });

  it("keeps a failed config update visible without changing the tree", async () => {
    mockCommands({ save_config: () => Promise.reject(new Error("disk full")) });

    await removeDestinationRoot("/existing");

    expect(useDestinationsStore.getState().roots).toEqual(["/existing"]);
    expect(inEnglish(useDestinationsStore.getState().message)).toBe(
      "Couldn’t remove that destination.",
    );
    expect(invokeCalls.find((call) => call.command === "save_config")?.args).toMatchObject({
      reportFailure: false,
    });
  });

  it("serializes root edits so a delayed save cannot overwrite a later intent", async () => {
    let finishFirst: (() => void) | undefined;
    let saves = 0;
    mockCommands({
      save_config: ({ changes }) => {
        saves += 1;
        const saved = savedConfig(changes);
        if (saves === 1) {
          return new Promise<Record<string, unknown>>((resolve) => {
            finishFirst = () => resolve(saved);
          });
        }
        return saved;
      },
    });
    openDialog.mockResolvedValueOnce("/added");

    const add = addDestinationRoot();
    await settle();
    const remove = removeDestinationRoot("/existing");
    await settle();
    expect(saves).toBe(1);

    finishFirst?.();
    await Promise.all([add, remove]);

    expect(useDestinationsStore.getState().roots).toEqual(["/added"]);
    expect(
      invokeCalls.filter((call) => call.command === "save_config").map((call) =>
        call.args.changes,
      ),
    ).toEqual([
      { destinationRoots: ["/existing", "/added"] },
      { destinationRoots: ["/added"] },
    ]);
  });
});

// main-review.md: "Dragging an unselected item exclusively selects and drags
// it. Crossing the drag threshold from a selected member drags the complete
// selection." (R5.1 D9)
describe("destination drag selection scope", () => {
  beforeEach(() => {
    mockCommands({ activity_record: () => null, record_recent_notification: () => ({}) });
    useItemsStore.setState({
      selected: { kind: "image", month: "2026-01" },
      items: [item(1), item(2), item(3), item(4)],
      totalItems: 4,
      windowStart: 0,
      itemPositions: new Map([["h1", 0], ["h2", 1], ["h3", 2], ["h4", 3]]),
      selectedItem: "h2",
      selectedKeys: new Set(["h1", "h2", "h3"]),
      selectedPositions: new Map([["h1", 0], ["h2", 1], ["h3", 2]]),
    });
  });

  it("exclusively selects and drags an unselected item, leaving the prior selection behind", () => {
    const dragged = beginDestinationDrag("h4");

    expect(dragged?.items).toEqual([{ hash: "h4", pathId: null }]);
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["h4"]);
    expect(useItemsStore.getState().selectedItem).toBe("h4");
  });

  it("carries the whole selection when the drag starts from a selected member", () => {
    const dragged = beginDestinationDrag("h2");

    expect(dragged?.items.map((entry) => entry.hash)).toEqual(["h1", "h2", "h3"]);
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["h1", "h2", "h3"]);
    // Starting the drag from a selected member never changes the anchor.
    expect(useItemsStore.getState().selectedItem).toBe("h2");
  });
});

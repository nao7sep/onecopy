// @vitest-environment happy-dom
//
// The frontend journeys (Phase 28): full user paths ACROSS stores and
// components together, through the mocked IPC layer — the wiring BETWEEN
// stores that the per-unit specs structurally cannot see. The app is rendered
// whole; the journey is driven by the same events a user produces (backend
// events, sidebar clicks, window and grid keystrokes), and every assertion is
// about what the user would see or what the core was actually told.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import App from "../../src/App";
import { useWizardStore } from "../../src/state/wizard-store";
import { useAppStore } from "../../src/state/app-store";
import { useSectionsStore } from "../../src/state/sections-store";
import { useItemsStore } from "../../src/state/items-store";
import { usePreviewStore } from "../../src/state/preview-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import { useComparisonStore } from "../../src/state/comparison-store";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import {
  fireEvent,
  invokeCalls,
  mockCommand,
  mockCommands,
  mockSectionItems,
  resetTauriMocks,
} from "../mocks/tauri";
import { finishWizard } from "../../src/workflows/wizard";

function item(pathId: number, over: Partial<SectionItem> = {}): SectionItem {
  return {
    hash: `h${pathId}`,
    pathId,
    fileName: `IMG_${pathId}.jpg`,
    resolvedUtcMs: pathId * 1000,
    copyCount: 1,
    width: 100,
    height: 100,
    hasThumb: false,
    similarGroupId: null,
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

function member(hash: string, sharpness: number) {
  return {
    hash,
    fileName: `${hash}.jpg`,
    width: 100,
    height: 100,
    byteSize: 1000,
    sharpness,
    faceScore: null,
    copyCount: 1,
    hasThumb: true,
  };
}

/** The scene the journey culls: three similar shots and one unrelated. */
const SCENE = [
  item(1, { similarGroupId: 7 }),
  item(2, { similarGroupId: 7 }),
  item(3, { similarGroupId: 7 }),
  item(4),
];

const pressWindow = (key: string, init: KeyboardEventInit = {}) =>
  window.dispatchEvent(
    new KeyboardEvent("keydown", { key, bubbles: true, ...init }),
  );

/** One macrotask drain — lets workflow-coordinated, void-awaited refreshes
 * land before the next assertion. */
const settle = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  let sourceCheckSnapshot = {
    running: false,
    stopping: false,
    lastResult: "stopped",
    eventSequence: 0,
  };
  mockCommands({
    load_app_data: () => ({
      config: { sourceDirs: [], defaultTimezone: "UTC" },
      state: {},
      dataRoot: "/data",
      debugEnabled: false,
    }),
    get_section_counts: () => ({ images: [], videos: [], others: [] }),
    get_issues: () => ({ issues: [], total: 0 }),
    binaries_state: () => [],
    check_source_dirs: () => ({ missing: [], substituted: [] }),
    patch_state: () => ({}),
    patch_config: (args) => args.patch ?? {},
    log_event: () => null,
    logging_debug_enabled: () => false,
    background_work_snapshot: () => ({
      masterPaused: false,
      classes: [],
      activeItem: null,
    }),
    index_work_snapshot: () => ({
      sourceCheck: sourceCheckSnapshot,
      fileInformation: {
        running: false,
        paused: false,
        stopping: false,
        queued: false,
        eventSequence: 0,
      },
    }),
    get_item_detail: () => null,
    start_source_check: () => {
      sourceCheckSnapshot = {
        running: true,
        stopping: false,
        lastResult: "stopped",
        eventSequence: sourceCheckSnapshot.eventSequence + 1,
      };
      return true;
    },
    admit_background_completion: () => null,
  });
  // Journeys start clean; module-load listeners survive resetTauriMocks.
  useWizardStore.setState({
    open: false,
    dirs: [],
    timezone: "UTC",
    timezoneValid: true,
    error: null,
  });
  useAppStore.setState({ appData: null, loadError: null, quarantines: [] });
  useSectionsStore.setState({
    counts: null,
    sourceCheck: {
      running: false,
      stopping: false,
      lastResult: "stopped",
      eventSequence: 0,
      progress: null,
    },
    fileInformation: {
      running: false,
      paused: false,
      stopping: false,
      queued: false,
      eventSequence: 0,
      progress: null,
    },
    rescanNeeded: false,
  });
  useItemsStore.setState({
    selected: null,
    items: [],
    loading: false,
    selectedItem: null,
    selectedKeys: new Set(),
    detail: null,
    message: null,
  });
  usePreviewStore.setState({ follow: false, current: null });
  useQuickViewStore.setState({ session: null, pendingDelete: null });
  useComparisonStore.setState({
    open: false,
    members: [],
    selected: new Set(),
    anchors: new Set(),
    anchor: null,
  });
});

afterEach(() => cleanup());

describe("the culling journey", () => {
  it("starts the configured source check only after usable bootstrap", async () => {
    mockCommand("load_app_data", () => ({
      config: {
        sourceDirs: ["/photos"],
        defaultTimezone: "UTC",
        checkSourceFoldersAtLaunch: true,
      },
      state: {},
      dataRoot: "/data",
      debugEnabled: false,
    }));

    render(<App />);
    await settle();
    await settle();

    expect(invokeCalls.map((call) => call.command)).toContain("check_source_dirs");
    expect(invokeCalls.map((call) => call.command)).toContain("start_source_check");
    expect(invokeCalls.map((call) => call.command)).not.toContain(
      "admit_background_completion",
    );
  });

  it("admits pending file information directly when the launch source check is off", async () => {
    mockCommand("load_app_data", () => ({
      config: {
        sourceDirs: ["/photos"],
        defaultTimezone: "UTC",
        checkSourceFoldersAtLaunch: false,
      },
      state: {},
      dataRoot: "/data",
      debugEnabled: false,
    }));

    render(<App />);
    await settle();
    await settle();

    expect(invokeCalls.map((call) => call.command)).not.toContain("start_source_check");
    expect(invokeCalls.map((call) => call.command)).toContain(
      "admit_background_completion",
    );
  });

  it("runs wizard finish → scan → tree → month → arrows → Space → Enter → page decision → refresh", async () => {
    const view = render(<App />);
    await settle();

    // ---- Wizard finish starts the scan through the real finish flow ----
    await act(async () => {
      useWizardStore.setState({
        open: true,
        dirs: [{ path: "/photos" }],
        timezone: "UTC",
        timezoneValid: true,
      });
      await finishWizard();
    });
    expect(invokeCalls.map((c) => c.command)).toContain("patch_config");
    expect(invokeCalls.map((c) => c.command)).toContain("start_source_check");
    expect(useSectionsStore.getState().sourceCheck.running).toBe(true);

    // ---- Scan events: progress is visible, done populates the tree ----
    await act(async () => {
      fireEvent("source-check://progress", {
        eventSequence: 2,
        progress: {
          phase: "walk",
          done: 0,
          total: 1,
          currentPath: "/photos",
          discovered: 4,
          bytesDone: null,
          bytesTotal: null,
          failures: 0,
          nextPhase: "hash",
        },
      });
    });
    expect(useSectionsStore.getState().sourceCheck.progress?.phase).toBe(
      "walk",
    );

    mockCommand("get_section_counts", () => ({
      images: [{ month: "2026-01", count: 4 }],
      videos: [],
      others: [],
    }));
    mockSectionItems(() => SCENE);
    await act(async () => {
      fireEvent("source-check://done", { eventSequence: 3 });
    });
    await settle();
    expect(useSectionsStore.getState().sourceCheck.running).toBe(false);

    // The sidebar tree: kind rows are open by default, years are closed —
    // the year appears, the month only after the year is opened.
    const rowByText = (text: string) =>
      [
        ...view.container.querySelectorAll<HTMLElement>(
          "[role='treeitem'], button",
        ),
      ].find((el) => el.textContent?.trim().startsWith(text));
    expect(rowByText("2026")).toBeTruthy();
    expect(rowByText("2026-01")).toBeFalsy();
    await act(async () => rowByText("2026")!.click());
    const monthRow = rowByText("2026-01");
    expect(monthRow).toBeTruthy();

    // ---- Opening the month loads its items into the grid ----
    await act(async () => monthRow!.click());
    await settle();
    expect(useItemsStore.getState().items).toHaveLength(4);
    const grid = view.container.querySelector<HTMLElement>("[role='listbox']")!;
    expect(grid).toBeTruthy();

    // ---- Arrows move the anchor ----
    await act(async () => {
      useItemsStore.getState().selectItem("h1");
    });
    await act(async () => {
      grid.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
      );
    });
    expect(useItemsStore.getState().selectedItem).toBe("h2");

    // ---- Space opens transient Quick View without changing Preview ----
    await act(async () => {
      grid.dispatchEvent(
        new KeyboardEvent("keydown", { key: " ", bubbles: true }),
      );
    });
    await settle();
    expect(useQuickViewStore.getState().session?.presentation).toBe("quick");
    expect(usePreviewStore.getState().follow).toBe(false);
    await act(async () => pressWindow("Escape"));
    await settle();
    expect(useQuickViewStore.getState().session).toBeNull();

    // ---- Enter on a ≈ item opens the comparison over the whole scene ----
    mockCommand("get_similar_group", () => [
      member("h2", 9),
      member("h1", 5),
      member("h3", 1),
    ]);
    await act(async () => {
      grid.focus();
      grid.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });
    await settle();
    const comparison = useComparisonStore.getState();
    expect(comparison.open).toBe(true);
    expect(comparison.members.map((m) => m!.hash)).toEqual(["h2", "h1", "h3"]);

    // ---- Entry selection + Enter decides the visible page ----
    const deleted: string[] = [];
    mockCommand("delete_items", (args) => {
      const items = args.items as Array<{
        hash: string | null;
        pathId: number | null;
      }>;
      deleted.push(...items.map((item) => item.hash!).filter(Boolean));
      return {
        cancelled: false,
        error: null,
        failedFiles: 0,
        items: items.map((item) => ({ item, failedFiles: 0 })),
      };
    });
    mockSectionItems(() => [
      item(2, { similarGroupId: null }),
      item(4),
    ]);
    expect(useComparisonStore.getState().selected.has("h2")).toBe(true);
    await act(async () => {
      pressWindow("Enter");
    });
    await settle();

    // The core was told to trash exactly the non-selected slots, the comparison
    // closed (nothing left to decide), and the month view refreshed to what
    // survived.
    expect(deleted.sort()).toEqual(["h1", "h3"]);
    expect(useComparisonStore.getState().open).toBe(false);
    expect(useItemsStore.getState().items.map((i) => i.hash)).toEqual([
      "h2",
      "h4",
    ]);
  }, 30_000);
});

describe("the failure journey", () => {
  it("surfaces a disk-failed delete in the status bar and Issues without breaking the selection", async () => {
    // A CONFIGURED boot — with source dirs the wizard stays closed, so the
    // window command layer (Delete) is live.
    mockCommand("load_app_data", () => ({
      config: { sourceDirs: ["/photos"], defaultTimezone: "UTC" },
      state: {},
      dataRoot: "/data",
      debugEnabled: false,
    }));
    mockSectionItems(() => SCENE);
    const view = render(<App />);
    await settle();

    // A loaded month with the anchor on the first item.
    await act(async () => {
      await useItemsStore
        .getState()
        .select({ kind: "image", month: "2026-01" });
    });
    await act(async () => {
      useItemsStore.getState().selectItem("h1");
    });

    // The disk says no: the outcome reports the failure, never a rejection,
    // and the issues surface starts carrying it.
    mockCommand("delete_items", ({ items }) => ({
      cancelled: false,
      error: null,
      failedFiles: 1,
      items: [{ item: (items as unknown[])[0], failedFiles: 1 }],
    }));
    mockCommand("get_issues", () => ({
      issues: [
        {
          id: 1,
          kind: "delete-failed",
          path: "/photos/IMG_1.jpg",
          message: "read-only volume",
          firstSeenUtc: "2026-01-01T00:00:00.000Z",
          lastSeenUtc: "2026-01-01T00:00:00.000Z",
          occurrenceCount: 1,
          recovery: null,
        },
      ],
      total: 1,
    }));
    await act(async () => {
      const grid =
        view.container.querySelector<HTMLElement>("[role='listbox']")!;
      grid.focus();
      grid.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Delete", bubbles: true }),
      );
    });
    await settle();

    // Status bar: the failure message AND the danger-tinted issues count.
    expect(view.container.textContent).toContain("could not be deleted");
    expect(view.container.textContent).toContain("1 issue");

    // The selection survived the failed delete: recovery moved the anchor to
    // a neighbour instead of dropping it, so the keyboard flow never breaks.
    const { selectedItem, selectedKeys } = useItemsStore.getState();
    expect(selectedItem).not.toBeNull();
    expect(selectedKeys.size).toBeGreaterThan(0);
  });
});

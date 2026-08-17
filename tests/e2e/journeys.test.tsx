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
import { useSectionsStore } from "../../src/state/sections-store";
import { useItemsStore } from "../../src/state/items-store";
import { usePreviewStore } from "../../src/state/preview-store";
import { useComparisonStore } from "../../src/state/comparison-store";
import type { SectionItem } from "../../src/models/items";
import {
  fireEvent,
  invokeCalls,
  mockCommand,
  mockCommands,
  resetTauriMocks,
} from "../mocks/tauri";

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
    byteSize: 1000,
    hasCompanions: false,
    durationMs: null,
    dirPaths: ["/photos"],
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
  window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, ...init }));

/** One macrotask drain — lets the stores' fire-and-forget dynamic imports and
 * void-awaited refreshes land before the next assertion. */
const settle = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
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
    get_item_detail: () => null,
    start_scan: () => true,
  });
  // Journeys start clean; module-load listeners survive resetTauriMocks.
  useWizardStore.setState({ open: false, dirs: [], timezone: "UTC", timezoneValid: true, cacheDir: null });
  useSectionsStore.setState({ counts: null, scanning: false, progress: "", rescanNeeded: false });
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
  useComparisonStore.setState({ open: false, slots: [], queue: [], kept: new Set() });
});

afterEach(() => cleanup());

describe("the culling journey", () => {
  it("runs wizard finish → scan → tree → month → arrows → Space → Enter → keeper commit → refresh", async () => {
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
      await useWizardStore.getState().finish();
    });
    expect(invokeCalls.map((c) => c.command)).toContain("patch_config");
    expect(invokeCalls.map((c) => c.command)).toContain("start_scan");
    expect(useSectionsStore.getState().scanning).toBe(true);

    // ---- Scan events: progress is visible, done populates the tree ----
    await act(async () => {
      fireEvent("scan://progress", { phase: "walk", detail: "/photos: 4 files" });
    });
    expect(useSectionsStore.getState().progress).toContain("Scanning");

    mockCommand("get_section_counts", () => ({
      images: [{ month: "2026-01", count: 4 }],
      videos: [],
      others: [],
    }));
    mockCommand("get_section_items", () => SCENE);
    await act(async () => {
      fireEvent("scan://done", {});
    });
    await settle();
    expect(useSectionsStore.getState().scanning).toBe(false);

    // The sidebar tree: kind rows are open by default, years are closed —
    // the year appears, the month only after the year is opened.
    const rowByText = (text: string) =>
      [...view.container.querySelectorAll<HTMLElement>("[role='treeitem'], button")].find(
        (el) => el.textContent?.trim().startsWith(text),
      );
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
      grid.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    });
    expect(useItemsStore.getState().selectedItem).toBe("h2");

    // ---- Space = look: the preview follows the anchor ----
    await act(async () => {
      grid.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true }));
    });
    await settle();
    expect(usePreviewStore.getState().follow).toBe(true);

    // ---- Enter on a ≈ item opens the comparison over the whole scene ----
    mockCommand("get_similar_group", () => [
      member("h2", 9),
      member("h1", 5),
      member("h3", 1),
    ]);
    await act(async () => {
      pressWindow("Enter");
    });
    await settle();
    const comparison = useComparisonStore.getState();
    expect(comparison.open).toBe(true);
    expect(comparison.slots.map((s) => s!.hash)).toEqual(["h2", "h1", "h3"]);

    // ---- Keeper key + Enter commits: losers to trash, view refreshes ----
    const deleted: string[] = [];
    mockCommand("delete_item", (args) => {
      deleted.push(args.hash as string);
      return { deletedFiles: 1, failedFiles: 0, removedRows: 1 };
    });
    mockCommand("get_section_items", () => [
      item(2, { similarGroupId: null }),
      item(4),
    ]);
    await act(async () => {
      pressWindow("1"); // keep slot 1 (h2, the sharpest)
    });
    expect(useComparisonStore.getState().kept.has("h2")).toBe(true);
    await act(async () => {
      pressWindow("Enter");
    });
    await settle();

    // The core was told to trash exactly the non-kept slots, the comparison
    // closed (nothing left to decide), and the month view refreshed to what
    // survived.
    expect(deleted.sort()).toEqual(["h1", "h3"]);
    expect(useComparisonStore.getState().open).toBe(false);
    expect(useItemsStore.getState().items.map((i) => i.hash)).toEqual(["h2", "h4"]);
  });
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
    mockCommand("get_section_items", () => SCENE);
    const view = render(<App />);
    await settle();

    // A loaded month with the anchor on the first item.
    await act(async () => {
      await useItemsStore.getState().select({ kind: "image", month: "2026-01" });
    });
    await act(async () => {
      useItemsStore.getState().selectItem("h1");
    });

    // The disk says no: the outcome reports the failure, never a rejection,
    // and the issues surface starts carrying it.
    mockCommand("delete_item", () => ({ deletedFiles: 0, failedFiles: 1, removedRows: 0 }));
    mockCommand("get_issues", () => ({
      issues: [
        {
          id: 1,
          kind: "delete-failed",
          path: "/photos/IMG_1.jpg",
          message: "read-only volume",
          firstSeenUtc: "2026-01-01T00:00:00.000Z",
          lastSeenUtc: "2026-01-01T00:00:00.000Z",
        },
      ],
      total: 1,
    }));
    await act(async () => {
      pressWindow("Delete");
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

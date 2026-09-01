// @vitest-environment happy-dom
//
// The grid's keyboard layer. Every key here moves the anchor the metadata
// pane, the preview follow and the delete all read, so a wrong target is not
// a navigation annoyance — it is the wrong photo selected for a destructive
// action.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act, fireEvent } from "@testing-library/react";
import Grid from "../../src/components/Grid";
import { useItemsStore } from "../../src/state/items-store";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { usePreviewStore } from "../../src/state/preview-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import {
  invokeCalls,
  mockCommands,
  mockSectionItems,
  resetTauriMocks,
} from "../mocks/tauri";
import { useDerivedWorkStore } from "../../src/state/derived-work-store";
import { useSectionsStore } from "../../src/state/sections-store";
import { useDestinationsStore } from "../../src/state/destinations-store";

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
    byteSize: pathId * 10,
    hasCompanions: false,
    durationMs: null,
    dirPaths: ["/photos"],
    derivedWork: EMPTY_ITEM_WORK,
    ...over,
  };
}

const ITEMS = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => item(n));

function renderGrid(
  items = ITEMS,
  loading = false,
  layout: "tiles" | "list" = "tiles",
  loadError: string | null = null,
) {
  useItemsStore.setState({
    items,
    totalItems: items.length,
    windowStart: 0,
    itemPositions: new Map(items.map((entry, index) => [entry.hash!, index])),
  });
  const view = render(
    <Grid
      items={items}
      loading={loading}
      loadError={loadError}
      layout={layout}
    />,
  );
  const container = view.container.querySelector<HTMLElement>("[role='listbox']");
  return { view, container: container! };
}

/** Sets the anchor and lets React re-render before the next keypress.
 *
 * The grid's key handler closes over the anchor from its render, so a
 * synchronous dispatch straight after a store write would still see the
 * previous value — an artefact of driving it faster than a user can. */
async function anchor(key: string): Promise<void> {
  await act(async () => {
    useItemsStore.getState().selectItem(key);
  });
}

function press(container: HTMLElement, key: string, init: KeyboardEventInit = {}) {
  container.dispatchEvent(
    new KeyboardEvent("keydown", { key, bubbles: true, ...init }),
  );
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({
    patch_state: () => ({}),
    get_item_detail: () => null,
    get_section_counts: () => [],
  });
  mockSectionItems(() => ITEMS);
  usePreviewStore.setState({
    follow: false,
    placement: null,
    placementPreference: null,
    current: null,
  });
  useQuickViewStore.setState({ session: null, pendingDelete: null });
  useDerivedWorkStore.setState({ activeItem: null });
  useSectionsStore.setState({
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
  });
  useDestinationsStore.setState({
    dragSelection: null,
  });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: ITEMS,
    loading: false,
    loadError: null,
    selectedItem: null,
    selectedKeys: new Set(),
    selectedPositions: new Map(),
    totalItems: ITEMS.length,
    windowStart: 0,
    itemPositions: new Map(ITEMS.map((entry, index) => [entry.hash!, index])),
    rangeOrigin: null,
    rangeBase: new Set(),
    sectionMemory: {},
    scrollRequest: null,
    detail: null,
    sortOrders: { media: { order: "time", desc: false }, other: { order: "name", desc: false } },
    message: null,
  });
});

describe("section repair admission", () => {
  it("does not admit a second section repair while indexing owns the runtime", () => {
    useSectionsStore.setState({
      sourceCheck: {
        running: true,
        stopping: false,
        lastResult: "stopped",
        eventSequence: 1,
        progress: null,
      },
    });
    renderGrid();

    const button = [...document.querySelectorAll("button")].find(
      (candidate) => candidate.textContent === "Unavailable while checking source folders",
    );
    expect(button?.disabled).toBe(true);
  });
});

describe("section state", () => {
  it("distinguishes loading, failure, and an ordinary empty section", () => {
    const loading = renderGrid([], true);
    expect(loading.view.container.textContent).toContain("Loading…");
    loading.view.unmount();

    const failed = renderGrid([], false, "tiles", "Couldn’t load this section.");
    expect(failed.view.container.textContent).toContain("Couldn’t load this section.");
    expect(failed.view.container.textContent).not.toContain("Nothing in this section");
    failed.view.unmount();

    const empty = renderGrid([]);
    expect(empty.view.container.textContent).toContain("Nothing in this section");
  });

  it("keeps stale rows and reports a failed refresh", () => {
    const { view } = renderGrid(ITEMS, false, "tiles", "Couldn’t load this section.");
    expect(view.container.querySelectorAll("[role='option']").length).toBeGreaterThan(0);
    expect(view.container.textContent).toContain("Couldn’t load this section.");
  });
});

afterEach(() => cleanup());

describe("arrow navigation", () => {
  it("moves the anchor one tile with Right", async () => {
    const { container } = renderGrid();
    await anchor("h2");

    press(container, "ArrowRight");

    expect(useItemsStore.getState().selectedItem).toBe("h3");
  });

  it("does not select item zero while a reload is in flight", async () => {
    // The mechanism behind "the grid jumped to the top for no reason": a null
    // anchor used to clamp to index 0, so any arrow during a refresh both
    // moved the selection and scrolled away from where the user was.
    const { container } = renderGrid(ITEMS, true);
    await anchor("h5");

    press(container, "ArrowRight");

    expect(useItemsStore.getState().selectedItem).not.toBe("h1");
  });
});

describe("Home and End", () => {
  it("jump to the ends of the DISPLAYED order", async () => {
    await act(async () => useItemsStore.setState({ sortOrders: { media: { order: "size", desc: true }, other: { order: "name", desc: false } } }));
    // The bounded backend, not Grid, supplies the active total order.
    const { container } = renderGrid([...ITEMS].reverse());
    await anchor("h4");

    press(container, "Home");
    // Sorted by size descending, the largest is h8.
    expect(useItemsStore.getState().selectedItem).toBe("h8");

    press(container, "End");
    expect(useItemsStore.getState().selectedItem).toBe("h1");
  });
});

describe("Space", () => {
  it("opens Quick View without changing persistent Preview or selection", async () => {
    // It used to toggle the anchor in and out of the multi-selection, which
    // nobody found and which made Space a way to silently DESELECT the photo
    // about to be deleted. Selection stays put now; Space only opens Quick View.
    const { container } = renderGrid();
    await anchor("h3");

    await act(async () => press(container, " "));

    expect(useQuickViewStore.getState().session?.presentation).toBe("quick");
    expect(usePreviewStore.getState().follow).toBe(false);
    expect(useItemsStore.getState().selectedKeys.has("h3")).toBe(true);
    expect(useItemsStore.getState().selectedItem).toBe("h3");

    // A second Space is not a hidden Preview toggle.
    await act(async () => press(container, " "));
    expect(useQuickViewStore.getState().session?.presentation).toBe("quick");
    expect(usePreviewStore.getState().follow).toBe(false);
  });

  it("never reaches a delete", async () => {
    const { container } = renderGrid();
    await anchor("h3");

    await act(async () => press(container, " "));

    expect(invokeCalls.some((c) => c.command === "delete_items")).toBe(false);
  });
});

describe("pointer selection", () => {
  it("ordinary click is exclusive and clicking it again keeps it selected", () => {
    const { view } = renderGrid();
    const tile = view.container.querySelector<HTMLElement>("[data-item-key='h1'] figure")!;

    fireEvent.click(tile, { detail: 1 });
    expect(useItemsStore.getState().selectedKeys.has("h1")).toBe(true);
    fireEvent.click(tile, { detail: 1 });
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["h1"]);
  });

  it("Cmd/Ctrl-click toggles without disturbing the other selection", () => {
    const { view } = renderGrid();
    const first = view.container.querySelector<HTMLElement>("[data-item-key='h1'] figure")!;
    const second = view.container.querySelector<HTMLElement>("[data-item-key='h2'] figure")!;
    fireEvent.click(first, { detail: 1 });
    fireEvent.click(second, { detail: 1, ctrlKey: true });
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["h1", "h2"]);
    fireEvent.click(second, { detail: 1, ctrlKey: true });
    expect([...useItemsStore.getState().selectedKeys]).toEqual(["h1"]);
  });

  it("double-click exclusively selects and opens Quick View", async () => {
    const { view } = renderGrid();
    const tile = view.container.querySelector<HTMLElement>("[data-item-key='h1'] figure")!;

    fireEvent.click(tile, { detail: 1 });
    fireEvent.click(tile, { detail: 2 });
    fireEvent.doubleClick(tile, { detail: 2 });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(useItemsStore.getState().selectedKeys.has("h1")).toBe(true);
    expect(useQuickViewStore.getState().session?.presentation).toBe("quick");
  });
});

describe("thumbnail information hierarchy", () => {
  it("puts duration in the facts line and relationships in one labelled badge", () => {
    const related = [
      item(1, {
        copyCount: 3,
        similarGroupId: 7,
        hasCompanions: true,
        durationMs: 65_000,
        byteSize: 2_097_152,
        width: 1920,
        height: 1080,
      }),
      item(2, { similarGroupId: 7 }),
    ];
    const { view } = renderGrid(related);

    expect(view.container.textContent).toContain("1920×1080 · 1:05 · 2 MB");
    const accessibleLabel = view.getByText(
      "3 exact copies; 2 similar photos; has paired companion files; every action includes them",
    );
    expect(accessibleLabel.classList.contains("sr-only")).toBe(true);
    expect(accessibleLabel.previousElementSibling?.textContent).toBe("×3 · ≈2 · pair");
  });

  it("projects a check for one selected item and ordinals for a multi-selection", async () => {
    const { view } = renderGrid();
    await anchor("h1");
    expect(view.getByText("Selected").classList.contains("sr-only")).toBe(true);

    await act(async () => {
      useItemsStore.setState({
        selectedItem: "h2",
        selectedKeys: new Set(["h1", "h2"]),
      });
    });
    expect(view.getByText("Selected 1 of 2").previousElementSibling?.textContent).toBe("1");
    expect(view.getByText("Selected 2 of 2").previousElementSibling?.textContent).toBe("2");
  });

  it("overlays the one active runtime state without changing durable item facts", () => {
    const running = item(1, {
      derivedWork: {
        ...EMPTY_ITEM_WORK,
        faces: {
          state: "pending",
          hasValue: false,
          reason: null,
          done: null,
          total: null,
        },
      },
    });
    useDerivedWorkStore.setState({
      activeItem: {
        id: "faces",
        hash: "h1",
        done: 42,
        total: 100,
        stopping: false,
      },
    });
    const { view } = renderGrid([running]);
    expect(view.getByText("Face scoring running 42%").previousElementSibling?.textContent).toBe(
      "Faces 42%",
    );
    expect(running.derivedWork.faces?.state).toBe("pending");
  });
});

describe("other files", () => {
  it("render as rows, and Down moves ONE row rather than a tile column", async () => {
    // A list is one column by definition; measuring tiles would compute a
    // column count the layout does not have, and Down would skip files.
    const { container } = renderGrid(ITEMS, false, "list");
    await anchor("h2");

    press(container, "ArrowDown");

    expect(useItemsStore.getState().selectedItem).toBe("h3");
  });
});

describe("shift+arrow", () => {
  it("extends and then narrows the range", async () => {
    const { container } = renderGrid();
    await anchor("h1");

    await act(async () => press(container, "ArrowRight", { shiftKey: true }));
    await act(async () => press(container, "ArrowRight", { shiftKey: true }));
    expect(useItemsStore.getState().selectedKeys.size).toBe(3);

    // Reversing must shrink — the gesture that silently did nothing before.
    await act(async () => press(container, "ArrowLeft", { shiftKey: true }));
    expect(useItemsStore.getState().selectedKeys.size).toBe(2);
  });
});

describe("focus ownership", () => {
  it("does not steal focus when section items arrive", () => {
    const { container } = renderGrid();
    expect(document.activeElement).toBe(document.body);
    expect(document.activeElement).not.toBe(container);
  });

  it("leaves an existing focus owner alone", () => {
    const outside = document.createElement("input");
    document.body.appendChild(outside);
    outside.focus();

    const { container } = renderGrid();

    expect(document.activeElement).toBe(outside);
    expect(document.activeElement).not.toBe(container);
    outside.remove();
  });

});

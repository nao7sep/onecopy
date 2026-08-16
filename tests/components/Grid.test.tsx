// @vitest-environment happy-dom
//
// The grid's keyboard layer. Every key here moves the anchor the metadata
// pane, the preview follow and the delete all read, so a wrong target is not
// a navigation annoyance — it is the wrong photo selected for a destructive
// action.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act } from "@testing-library/react";
import Grid from "../../src/components/Grid";
import { useItemsStore } from "../../src/state/items-store";
import type { SectionItem } from "../../src/models/items";
import { usePreviewStore } from "../../src/state/preview-store";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

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
    byteSize: pathId * 10,
    hasCompanions: false,
    durationMs: null,
    ...over,
  };
}

const ITEMS = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => item(n));

function renderGrid(items = ITEMS, loading = false, layout: "tiles" | "list" = "tiles") {
  const view = render(<Grid items={items} loading={loading} layout={layout} />);
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
    get_section_items: () => ITEMS,
    get_section_counts: () => [],
  });
  usePreviewStore.setState({
    follow: false,
    placement: null,
    placementPreference: null,
    screenCount: 1,
    current: null,
  });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: ITEMS,
    loading: false,
    selectedItem: null,
    selectedKeys: new Set(),
    rangeOrigin: null,
    rangeBase: new Set(),
    detail: null,
    sortOrder: "time",
    message: null,
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
    const { container } = renderGrid();
    // Sorted by size DESCENDING, so the display order reverses the fixtures.
    await act(async () => useItemsStore.setState({ sortOrder: "size" }));
    await anchor("h4");

    press(container, "Home");
    // Sorted by size descending, the largest is h8.
    expect(useItemsStore.getState().selectedItem).toBe("h8");

    press(container, "End");
    expect(useItemsStore.getState().selectedItem).toBe("h1");
  });
});

describe("Space", () => {
  it("is Quick Look: it shows the preview and leaves the selection alone", async () => {
    // It used to toggle the anchor in and out of the multi-selection, which
    // nobody found and which made Space a way to silently DESELECT the photo
    // about to be deleted. Selection stays put now; Space only previews.
    const { container } = renderGrid();
    await anchor("h3");

    await act(async () => press(container, " "));

    expect(usePreviewStore.getState().follow).toBe(true);
    expect(useItemsStore.getState().selectedKeys.has("h3")).toBe(true);
    expect(useItemsStore.getState().selectedItem).toBe("h3");

    // And again hides it — one key, both directions.
    await act(async () => press(container, " "));
    expect(usePreviewStore.getState().follow).toBe(false);
  });

  it("never reaches a delete", async () => {
    const { container } = renderGrid();
    await anchor("h3");

    await act(async () => press(container, " "));

    expect(invokeCalls.some((c) => c.command === "delete_item")).toBe(false);
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

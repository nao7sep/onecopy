// @vitest-environment happy-dom
//
// The grid's keyboard layer. Every key here moves the anchor the metadata
// pane, the preview follow and the delete all read, so a wrong target is not
// a navigation annoyance — it is the wrong photo selected for a destructive
// action.

import { beforeEach, afterEach, describe, expect, it } from "vitest";
import { render, cleanup, act, fireEvent } from "@testing-library/react";
import Grid from "../../src/components/Grid";
import { popModal, pushModal } from "../../src/utils/modalStack";
import { useItemsStore } from "../../src/state/items-store";
import type { SectionItem } from "../../src/models/items";
import { usePreviewStore } from "../../src/state/preview-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
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
    namesDiffer: false,
    dirPaths: ["/photos"],
    ...over,
  };
}

const ITEMS = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => item(n));

function renderGrid(
  items = ITEMS,
  loading = false,
  layout: "tiles" | "list" = "tiles",
  mayClaimFocus = true,
) {
  const view = render(
    <Grid items={items} loading={loading} layout={layout} mayClaimFocus={mayClaimFocus} />,
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
    get_section_items: () => ITEMS,
    get_section_counts: () => [],
  });
  usePreviewStore.setState({
    follow: false,
    placement: null,
    placementPreference: null,
    current: null,
  });
  useQuickViewStore.setState({ open: false });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: ITEMS,
    loading: false,
    selectedItem: null,
    selectedKeys: new Set(),
    rangeOrigin: null,
    rangeBase: new Set(),
    detail: null,
    sortOrders: { media: { order: "time", desc: false }, other: { order: "name", desc: false } },
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
    await act(async () => useItemsStore.setState({ sortOrders: { media: { order: "size", desc: true }, other: { order: "name", desc: false } } }));
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

    expect(useQuickViewStore.getState().open).toBe(true);
    expect(usePreviewStore.getState().follow).toBe(false);
    expect(useItemsStore.getState().selectedKeys.has("h3")).toBe(true);
    expect(useItemsStore.getState().selectedItem).toBe("h3");

    // A second Space is not a hidden Preview toggle.
    await act(async () => press(container, " "));
    expect(useQuickViewStore.getState().open).toBe(true);
    expect(usePreviewStore.getState().follow).toBe(false);
  });

  it("never reaches a delete", async () => {
    const { container } = renderGrid();
    await anchor("h3");

    await act(async () => press(container, " "));

    expect(invokeCalls.some((c) => c.command === "delete_item")).toBe(false);
  });
});

describe("pointer selection", () => {
  it("ordinary clicks toggle immediately", () => {
    const { view } = renderGrid();
    const tile = view.container.querySelector<HTMLElement>("[data-item-key='h1'] figure")!;

    fireEvent.click(tile, { detail: 1 });
    expect(useItemsStore.getState().selectedKeys.has("h1")).toBe(true);
    fireEvent.click(tile, { detail: 1 });
    expect(useItemsStore.getState().selectedKeys.has("h1")).toBe(false);
  });

  it("a double-click makes the toggle decision once", () => {
    const { view } = renderGrid();
    const tile = view.container.querySelector<HTMLElement>("[data-item-key='h1'] figure")!;

    fireEvent.click(tile, { detail: 1 });
    fireEvent.click(tile, { detail: 2 });
    fireEvent.doubleClick(tile, { detail: 2 });

    expect(useItemsStore.getState().selectedKeys.has("h1")).toBe(true);
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

describe("taking the keyboard on arrival", () => {
  // On boot the app reopens the last section by itself. Nobody clicked, so
  // focus was still on <body> and every arrow key went nowhere — the app read
  // as frozen until the user thought to click the grid.

  it("focuses itself when a restored section's items arrive", () => {
    const { container } = renderGrid();
    expect(document.activeElement).toBe(container);
  });

  it("waits for the items", () => {
    // Focusing an empty grid is not wrong, only useless — and it would fire
    // once per section change, stealing focus from wherever the user is.
    renderGrid([], true);
    expect(document.activeElement).toBe(document.body);
  });

  it("leaves focus alone when something else already has it", async () => {
    const outside = document.createElement("input");
    document.body.appendChild(outside);
    outside.focus();

    const { container } = renderGrid();

    expect(document.activeElement).toBe(outside);
    expect(document.activeElement).not.toBe(container);
    outside.remove();
  });

  it("does not reach through a modal", () => {
    const token = {};
    pushModal(token);
    try {
      renderGrid();
      expect(document.activeElement).toBe(document.body);
    } finally {
      popModal(token);
    }
  });

  it("does not reach through a boot gate", () => {
    // The wizard and the missing-volume gate are opaque overlays that focus
    // nothing themselves, so body stays active and only the flag stops it.
    renderGrid(ITEMS, false, "tiles", false);
    expect(document.activeElement).toBe(document.body);
  });
});

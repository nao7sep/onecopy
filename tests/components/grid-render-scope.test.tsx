// @vitest-environment happy-dom
//
// The grid re-renders only the cells whose own state changed. Each tile's
// thumbnail asks for its URL when it renders, so the count of those requests
// is the count of tile renders. The test viewport has no height, so only the
// first rows mount.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { mockCommands, mockFullscreenDisplay, mockSectionItems, resetTauriMocks } from "../mocks/tauri";
import { seedAppConfig } from "../helpers/config";

const thumbRenders = vi.hoisted(() => ({ count: 0 }));
vi.mock("../../src/models/items", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../src/models/items")>();
  return {
    ...actual,
    thumbUrl: (hash: string) => {
      thumbRenders.count += 1;
      return actual.thumbUrl(hash);
    },
  };
});

const { default: Grid } = await import("../../src/components/Grid");
const { EMPTY_ITEM_WORK } = await import("../../src/models/items");
const { useItemsStore } = await import("../../src/state/items-store");
const { useDerivedWorkStore } = await import("../../src/state/derived-work-store");
type SectionItem = import("../../src/models/items").SectionItem;

function item(pathId: number): SectionItem {
  return {
    hash: `h${pathId}`,
    pathId,
    fileName: `IMG_${pathId}.jpg`,
    resolvedUtcMs: pathId * 1000,
    copyCount: 1,
    width: 100,
    height: 100,
    hasThumb: true,
    similarGroupId: null,
    sharpness: null,
    faceScore: null,
    byteSize: pathId * 10,
    hasCompanions: false,
    durationMs: null,
    dirPaths: ["/photos"],
    derivedWork: EMPTY_ITEM_WORK,
  };
}

const ITEMS = [1, 2, 3, 4, 5, 6, 7, 8].map(item);

beforeEach(() => {
  seedAppConfig();
  resetTauriMocks({ keepListeners: true });
  mockFullscreenDisplay();
  mockCommands({ patch_state: () => ({}), get_item_detail: () => null, get_section_counts: () => [] });
  mockSectionItems(() => ITEMS);
  useDerivedWorkStore.setState({ activeItem: null });
  useItemsStore.setState({
    items: ITEMS,
    totalItems: ITEMS.length,
    windowStart: 0,
    itemPositions: new Map(ITEMS.map((entry, index) => [entry.hash!, index])),
    selectedKeys: new Set(),
    selectedItem: null,
  });
});

afterEach(() => cleanup());

function renderGrid() {
  const view = render(<Grid items={ITEMS} loading={false} loadError={null} layout="tiles" />);
  return view.container.querySelector<HTMLElement>("[role='listbox']")!;
}

describe("grid render scope", () => {
  it("re-renders only the selected tile when the selection changes", () => {
    renderGrid();
    const before = thumbRenders.count;
    act(() => useItemsStore.getState().selectItem("h1"));
    expect(thumbRenders.count - before).toBe(1);
  });

  it("re-renders only the tile whose background work progressed", () => {
    renderGrid();
    const before = thumbRenders.count;
    act(() => {
      useDerivedWorkStore.setState({
        activeItem: { id: "faces", hash: "h1", done: 1, total: 10, stopping: false },
      });
    });
    act(() => {
      useDerivedWorkStore.setState({
        activeItem: { id: "faces", hash: "h1", done: 2, total: 10, stopping: false },
      });
    });
    expect(thumbRenders.count - before).toBe(2);
  });

  it("does not re-render on a scroll that keeps the same rows in view", () => {
    const container = renderGrid();
    container.scrollTop = 1;
    fireEvent.scroll(container);
    const before = thumbRenders.count;
    container.scrollTop = 2;
    fireEvent.scroll(container);
    container.scrollTop = 3;
    fireEvent.scroll(container);
    expect(thumbRenders.count).toBe(before);
  });
});

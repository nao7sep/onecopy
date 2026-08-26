// The preview's placement and anchor contracts after the rework.
//
// Placement is purely the user's choice — the developer's explicit call:
// "app doesn't need to detect how many screens the system has for preview".
// Any monitor-derived rule makes some legitimate arrangement (two windows on
// halves of one screen; one window on one screen of three) impossible to ask
// for, which is exactly what happened: on a two-screen machine the old rule
// forced the window placement, opened it BEHIND the main window, and the
// developer reported the preview as blank.

import { beforeEach, describe, expect, it } from "vitest";
import {
  resolvePlacement,
  usePreviewStore,
} from "../../src/state/preview-store";
import {
  handleSpaceQuickView,
  useQuickViewStore,
} from "../../src/state/quick-view-store";
import { useItemsStore } from "../../src/state/items-store";
import type { SectionItem } from "../../src/models/items";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

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
    byteSize: 10,
    hasCompanions: false,
    durationMs: null,
    namesDiffer: false,
    dirPaths: [`/Volumes/A/photos`],
  };
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ patch_state: () => ({}), get_item_detail: () => null });
  usePreviewStore.setState({
    follow: false,
    placement: null,
    placementPreference: null,
    current: null,
  });
  useQuickViewStore.setState({ open: false });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: [item(1), item(2)],
    selectedItem: null,
    selectedKeys: new Set(),
    detail: null,
  });
});

describe("the Space model", () => {
  it("opens Quick View without changing persistent Preview", () => {
    useItemsStore.setState({ selectedItem: "h1", selectedKeys: new Set(["h1"]) });
    let prevented = false;
    const claimed = handleSpaceQuickView({ preventDefault: () => (prevented = true) });
    expect(claimed).toBe(true);
    expect(prevented).toBe(true);
    expect(useQuickViewStore.getState().open).toBe(true);
    expect(usePreviewStore.getState().follow).toBe(false);
  });

  it("does nothing for non-media sections", () => {
    useItemsStore.setState({
      selected: { kind: "other", month: "2026-01" },
      selectedItem: "h1",
      selectedKeys: new Set(["h1"]),
    });
    let prevented = false;
    expect(handleSpaceQuickView({ preventDefault: () => (prevented = true) })).toBe(false);
    expect(prevented).toBe(false);
  });
});

describe("placement is the user's statement alone", () => {
  it("defaults to the in-window pane and honors the window choice", () => {
    expect(resolvePlacement(null)).toBe("split");
    expect(resolvePlacement("split")).toBe("split");
    expect(resolvePlacement("window")).toBe("window");
  });
});

describe("activating the preview", () => {
  it("shows the anchor's image IMMEDIATELY when one is selected", async () => {
    // The broken half the developer reported: activate-then-select was the
    // only order that worked. `open` must seed `current` before returning.
    useItemsStore.setState({ selectedItem: "h1", selectedKeys: new Set(["h1"]) });

    await usePreviewStore.getState().toggleFollow();

    const { follow, placement, current } = usePreviewStore.getState();
    expect(follow).toBe(true);
    expect(placement).toBe("split");
    expect(current?.hash).toBe("h1");
  });

  it("arms follow and stays empty when nothing is selected", async () => {
    await usePreviewStore.getState().toggleFollow();
    const { follow, current } = usePreviewStore.getState();
    expect(follow).toBe(true);
    expect(current).toBeNull();
  });

  it("a cleared selection blanks the surface instead of holding the last photo", async () => {
    useItemsStore.setState({ selectedItem: "h1", selectedKeys: new Set(["h1"]) });
    await usePreviewStore.getState().toggleFollow();
    expect(usePreviewStore.getState().current?.hash).toBe("h1");

    usePreviewStore.getState().anchorCleared();

    // Blank, not closed: follow stays armed for the next anchor.
    const { follow, current } = usePreviewStore.getState();
    expect(follow).toBe(true);
    expect(current?.hash).toBeNull();
  });

  it("a cleared selection never OPENS a closed preview", () => {
    // Restored follow with no surface yet: the first REAL anchor opens it, a
    // clear must not open an empty pane.
    usePreviewStore.setState({ follow: true, placement: null });
    usePreviewStore.getState().anchorChanged({ hash: null, pathId: null }, null);
    expect(usePreviewStore.getState().placement).toBeNull();
  });
});

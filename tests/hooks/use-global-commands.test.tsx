// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { useGlobalCommands } from "../../src/hooks/useGlobalCommands";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { useComparisonStore } from "../../src/state/comparison-store";
import { useItemsStore } from "../../src/state/items-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import {
  mockCommand,
  mockSectionItems,
  resetTauriMocks,
  setCurrentMonitor,
} from "../mocks/tauri";

const ITEM: SectionItem = {
  hash: "image-hash",
  pathId: 1,
  fileName: "image.jpg",
  resolvedUtcMs: 1,
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

function Harness() {
  useGlobalCommands();
  return (
    <>
      <div id="main-item-area" tabIndex={0} />
      <div aria-label="Preview pane" tabIndex={0} />
    </>
  );
}

beforeEach(() => {
  resetTauriMocks();
  mockSectionItems(() => [ITEM]);
  mockCommand("set_window_simple_fullscreen", () => null);
  setCurrentMonitor({
    position: { x: 0, y: 0 },
    size: { width: 1920, height: 1080 },
    workArea: {
      position: { x: 0, y: 0 },
      size: { width: 1920, height: 1040 },
    },
    scaleFactor: 2,
    name: "display",
  });
  useComparisonStore.setState({ open: false });
  useQuickViewStore.setState({ session: null, pendingDelete: null });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-01" },
    items: [ITEM],
    selectedItem: "image-hash",
    selectedKeys: new Set(["image-hash"]),
    selectedPositions: new Map([["image-hash", 0]]),
    totalItems: 1,
    windowStart: 0,
    itemPositions: new Map([["image-hash", 0]]),
    detail: null,
  });
});

afterEach(cleanup);

describe("global viewer commands", () => {
  it("opens true fullscreen when the in-pane Preview owns focus", async () => {
    const view = render(<Harness />);
    const preview = view.getByLabelText("Preview pane");
    preview.focus();

    fireEvent.keyDown(preview, { key: "f" });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(useQuickViewStore.getState().session?.presentation).toBe(
      "fullscreen",
    );
  });
});

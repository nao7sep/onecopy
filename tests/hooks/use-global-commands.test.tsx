// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { useGlobalCommands } from "../../src/hooks/useGlobalCommands";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { useComparisonStore } from "../../src/state/comparison-store";
import { useAppStore } from "../../src/state/app-store";
import { useItemsStore } from "../../src/state/items-store";
import { currentMainFeedback, useMainFeedbackStore } from "../../src/state/main-feedback-store";
import { useQuickViewStore } from "../../src/state/quick-view-store";
import { useAppShellStore } from "../../src/state/app-shell-store";
import { pushModal, resetModalStack } from "../../src/utils/modalStack";
import {
  fireEvent as fireBackendEvent,
  mockCommand,
  invokeCalls,
  mockSectionItems,
  resetTauriMocks,
  setCurrentMonitor,
} from "../mocks/tauri";
import { effectiveConfig } from "../helpers/config";

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
  const commands = useGlobalCommands();
  return (
    <>
      <div id="main-item-area" tabIndex={0} />
      <div aria-label="Preview pane" data-preview-pane tabIndex={0} />
      <div role="tree" aria-label="Sections" tabIndex={0} />
      <output aria-label="Trash confirmation">
        {commands.confirmTrash ?? "none"}
      </output>
      <output aria-label="Permanent confirmation">
        {commands.confirmPermanent ?? "none"}
      </output>
      <button type="button" onClick={commands.confirmTrashDelete}>
        Confirm trash
      </button>
      <button type="button" onClick={commands.confirmPermanentDelete}>
        Confirm permanent
      </button>
    </>
  );
}

beforeEach(() => {
  resetTauriMocks();
  mockSectionItems(() => [ITEM]);
  mockCommand("set_window_fullscreen", () => null);
  mockCommand("refresh_presentation_chrome", () => null);
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
  useAppStore.setState({
    appData: {
      config: effectiveConfig({ confirmTrashDelete: false }),
      state: {},
      dataRoot: "/app",
      debugEnabled: false,
      quarantines: [],
    },
  });
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

  it("does not open the viewer for F while the sidebar owns focus (R5.1 D4)", async () => {
    const view = render(<Harness />);
    const tree = view.getByRole("tree");
    tree.focus();

    fireEvent.keyDown(tree, { key: "f" });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    expect(useQuickViewStore.getState().session).toBeNull();
  });
});

describe("global destructive commands", () => {
  it("always reviews a multi-item Trash even when the single-item preference is off", () => {
    const second = { ...ITEM, hash: "image-hash-2", pathId: 2 };
    useItemsStore.setState({
      items: [ITEM, second],
      selectedKeys: new Set(["image-hash", "image-hash-2"]),
    });
    const view = render(<Harness />);
    const area = view.container.querySelector("#main-item-area")!;

    fireEvent.keyDown(area, { key: "Delete" });

    expect(view.getByLabelText("Trash confirmation").textContent).toBe("2");
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
  });

  // viewing-sessions.md D10: "persistent Preview" trashes Main's complete
  // selection from either placement, not only the separate window.
  it("trashes Main's selection from a Delete pressed inside the in-pane Preview", () => {
    useItemsStore.setState({ selectedItem: "image-hash", selectedKeys: new Set() });
    const view = render(<Harness />);
    const pane = view.getByLabelText("Preview pane");

    fireEvent.keyDown(pane, { key: "Delete" });

    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      true,
    );
  });

  // main-review.md: the configured single-item confirm preference applies to
  // exactly one selected item; a multi-item Delete always reviews regardless
  // of it (R5.1 D9's sibling contract, D11's single-item half).
  it("reviews a single-item Delete when the confirm-single-item preference is on", () => {
    useAppStore.setState({
      appData: {
        config: effectiveConfig({ confirmTrashDelete: true }),
        state: {},
        dataRoot: "/app",
        debugEnabled: false,
        quarantines: [],
      },
    });
    const view = render(<Harness />);
    const area = view.container.querySelector("#main-item-area")!;

    fireEvent.keyDown(area, { key: "Delete" });

    expect(view.getByLabelText("Trash confirmation").textContent).toBe("1");
    expect(invokeCalls.some((call) => call.command === "delete_items")).toBe(
      false,
    );
  });

  it("consumes repeated Enter and deletion without reopening or deleting", () => {
    const view = render(<Harness />);
    const area = view.container.querySelector("#main-item-area")!;

    fireEvent.keyDown(area, { key: "Enter", repeat: true });
    fireEvent.keyDown(area, { key: "Backspace", repeat: true });

    expect(
      invokeCalls.some((call) =>
        ["comparison_selection_valid", "delete_items"].includes(call.command),
      ),
    ).toBe(false);
    expect(view.getByLabelText("Trash confirmation").textContent).toBe(
      "none",
    );
  });

  it("permanently deletes the reviewed item, not the neighbour a refresh recovered to", async () => {
    mockCommand("delete_items", () => ({ error: null, failedFiles: 0 }));
    const view = render(<Harness />);
    const area = view.container.querySelector("#main-item-area")!;

    fireEvent.keyDown(area, { key: "Delete", shiftKey: true });
    expect(view.getByLabelText("Permanent confirmation").textContent).toBe("1");

    // A watcher refresh under the open review: the reviewed item left the
    // section and recovery selected its neighbour.
    useItemsStore.setState({
      selectedItem: "neighbour-hash",
      selectedKeys: new Set(["neighbour-hash"]),
      selectedPositions: new Map([["neighbour-hash", 0]]),
    });
    await act(async () => {
      fireEvent.click(view.getByText("Confirm permanent"));
    });

    const deletes = invokeCalls.filter((call) => call.command === "delete_items");
    expect(deletes).toHaveLength(1);
    expect(deletes[0]!.args).toEqual({
      items: [{ hash: "image-hash", pathId: null }],
      permanent: true,
    });
  });

  it("trashes exactly the reviewed items in their reviewed order", async () => {
    mockCommand("delete_items", () => ({ error: null, failedFiles: 0 }));
    useItemsStore.setState({
      selectedKeys: new Set(["image-hash-2", "image-hash"]),
      selectedPositions: new Map([
        ["image-hash", 0],
        ["image-hash-2", 1],
      ]),
    });
    const view = render(<Harness />);
    fireEvent.keyDown(view.container.querySelector("#main-item-area")!, { key: "Delete" });
    expect(view.getByLabelText("Trash confirmation").textContent).toBe("2");

    // A refresh drops one reviewed item and moves the other.
    useItemsStore.setState({
      selectedKeys: new Set(["image-hash-2", "unreviewed-hash"]),
      selectedPositions: new Map([
        ["unreviewed-hash", 0],
        ["image-hash-2", 1],
      ]),
    });
    await act(async () => {
      fireEvent.click(view.getByText("Confirm trash"));
    });

    const deletes = invokeCalls.filter((call) => call.command === "delete_items");
    expect(deletes).toHaveLength(1);
    expect(deletes[0]!.args).toEqual({
      items: [
        { hash: "image-hash", pathId: null },
        { hash: "image-hash-2", pathId: null },
      ],
      permanent: false,
    });
  });
});

describe("native Settings menu item (R8-04)", () => {
  afterEach(() => resetModalStack());

  it("opens Settings when the core reports the macOS menu item, unless a modal is already open", async () => {
    useAppShellStore.setState({ utilitySurface: null });
    render(<Harness />);

    await act(async () => {
      fireBackendEvent("menu://open-settings");
    });
    expect(useAppShellStore.getState().utilitySurface).toBe("settings");

    useAppShellStore.setState({ utilitySurface: null });
    pushModal({});
    await act(async () => {
      fireBackendEvent("menu://open-settings");
    });
    expect(useAppShellStore.getState().utilitySurface).toBeNull();
  });
});

describe("Enter playback toggle (R5.1 D3)", () => {
  it("does nothing, without an untrue notice, when no player is visible for a video anchor", () => {
    useItemsStore.setState({ selected: { kind: "video", month: "2026-01" } });
    const view = render(<Harness />);
    const area = view.container.querySelector("#main-item-area")!;

    fireEvent.keyDown(area, { key: "Enter" });

    expect(currentMainFeedback(useMainFeedbackStore.getState())).toBeNull();
  });
});

// @vitest-environment happy-dom

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  destinationReceiverAt,
  edgeScrollStep,
  useDestinationPointerDrag,
} from "../../src/hooks/useDestinationPointerDrag";
import DestinationDragPreview, {
  destinationDragPreviewPosition,
} from "../../src/components/DestinationDragPreview";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { useDestinationsStore } from "../../src/state/destinations-store";
import { useItemsStore } from "../../src/state/items-store";
import { cancelDestinationDrag } from "../../src/workflows/destinations";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

function item(): SectionItem {
  return {
    hash: "h1",
    pathId: 1,
    fileName: "portrait.png",
    resolvedUtcMs: 1,
    copyCount: 1,
    width: 100,
    height: 100,
    hasThumb: false,
    similarGroupId: null,
    sharpness: null,
    faceScore: null,
    byteSize: 100,
    hasCompanions: false,
    durationMs: null,
    namesDiffer: false,
    dirPaths: ["/library"],
    derivedWork: EMPTY_ITEM_WORK,
  };
}

function Harness({ onClick = () => undefined }: { onClick?: () => void }) {
  const drag = useDestinationPointerDrag({
    key: "h1",
    label: "portrait.png",
    thumbHash: null,
  });
  return (
    <>
      <div data-testid="source" onClick={onClick} {...drag.handlers}>
        Source
      </div>
      <div data-testid="receiver" data-destination-receiver="/keep">
        Keep
      </div>
      <DestinationDragPreview />
    </>
  );
}

const originalElementFromPoint = Object.getOwnPropertyDescriptor(
  document,
  "elementFromPoint",
);

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ log_event: () => null });
  useDestinationsStore.setState({
    dragSelection: null,
    dragReceiverPath: null,
    dragPresentation: null,
    pendingDrop: null,
  });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-08" },
    items: [item()],
    selectedItem: "h1",
    selectedKeys: new Set(["h1"]),
    rangeOrigin: "h1",
    rangeBase: new Set(["h1"]),
    sortOrders: {
      media: { order: "time", desc: false },
      other: { order: "name", desc: false },
    },
  });
});

afterEach(() => {
  cleanup();
  cancelDestinationDrag();
  if (originalElementFromPoint === undefined) {
    Reflect.deleteProperty(document, "elementFromPoint");
  } else {
    Object.defineProperty(document, "elementFromPoint", originalElementFromPoint);
  }
});

function pointAt(element: Element | null): void {
  Object.defineProperty(document, "elementFromPoint", {
    configurable: true,
    value: () => element,
  });
}

function begin(source: HTMLElement): void {
  fireEvent.pointerDown(source, {
    button: 0,
    isPrimary: true,
    pointerId: 7,
    clientX: 10,
    clientY: 10,
  });
}

describe("OneCopy's same-window destination pointer drag", () => {
  it("waits for movement, then highlights and commits the exact receiver", () => {
    const onClick = vi.fn();
    const view = render(<Harness onClick={onClick} />);
    const source = view.getByTestId("source");
    const receiver = view.getByTestId("receiver");
    pointAt(receiver);

    begin(source);
    fireEvent.pointerMove(document, {
      pointerId: 7,
      clientX: 13,
      clientY: 13,
    });
    expect(useDestinationsStore.getState().dragSelection).toBeNull();

    fireEvent.pointerMove(document, {
      pointerId: 7,
      clientX: 20,
      clientY: 10,
    });
    expect(useDestinationsStore.getState().dragReceiverPath).toBe("/keep");
    expect(view.getByText("portrait.png")).toBeTruthy();
    expect(view.getByText("Selected item")).toBeTruthy();

    fireEvent.pointerUp(document, {
      pointerId: 7,
      clientX: 20,
      clientY: 10,
    });
    expect(useDestinationsStore.getState().pendingDrop).toMatchObject({
      path: "/keep",
      selection: { items: [{ hash: "h1", pathId: null }] },
    });
    expect(useDestinationsStore.getState().dragSelection).toBeNull();
    expect(useDestinationsStore.getState().dragReceiverPath).toBeNull();
    expect(useDestinationsStore.getState().dragPresentation).toBeNull();

    fireEvent.click(source);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("cancels silently on Escape and outside release", () => {
    const view = render(<Harness />);
    const source = view.getByTestId("source");
    pointAt(view.getByTestId("receiver"));

    begin(source);
    fireEvent.pointerMove(document, {
      pointerId: 7,
      clientX: 20,
      clientY: 10,
    });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(useDestinationsStore.getState().dragSelection).toBeNull();
    expect(useDestinationsStore.getState().dragReceiverPath).toBeNull();
    expect(useDestinationsStore.getState().pendingDrop).toBeNull();

    pointAt(null);
    begin(source);
    fireEvent.pointerMove(document, {
      pointerId: 7,
      clientX: 20,
      clientY: 10,
    });
    fireEvent.pointerUp(document, {
      pointerId: 7,
      clientX: 20,
      clientY: 10,
    });
    expect(useDestinationsStore.getState().pendingDrop).toBeNull();
  });

  it("leaves an ordinary click alone below the drag threshold", () => {
    const onClick = vi.fn();
    const view = render(<Harness onClick={onClick} />);
    const source = view.getByTestId("source");
    pointAt(null);

    begin(source);
    fireEvent.pointerUp(document, {
      pointerId: 7,
      clientX: 12,
      clientY: 12,
    });
    fireEvent.click(source);

    expect(onClick).toHaveBeenCalledOnce();
    expect(useDestinationsStore.getState().dragSelection).toBeNull();
  });

  it("names a multi-selection without pretending Move or Copy was chosen", () => {
    useItemsStore.setState({
      items: [
        item(),
        { ...item(), hash: "h2", pathId: 2, fileName: "travel.jpg" },
      ],
      selectedKeys: new Set(["h1", "h2"]),
    });
    const view = render(<Harness />);
    pointAt(view.getByTestId("receiver"));

    begin(view.getByTestId("source"));
    fireEvent.pointerMove(document, {
      pointerId: 7,
      clientX: 20,
      clientY: 10,
    });

    expect(view.getByText("2 selected items")).toBeTruthy();
    expect(view.getByText("Includes portrait.png")).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/move|copy/i);
    fireEvent.pointerCancel(document, { pointerId: 7 });
  });
});

describe("semantic hit testing and bounded edge scrolling", () => {
  it("returns only a declared destination receiver", () => {
    const receiver = document.createElement("div");
    receiver.dataset.destinationReceiver = "/archive";
    const child = document.createElement("span");
    receiver.append(child);
    pointAt(child);

    expect(destinationReceiverAt(20, 20)).toBe("/archive");
    pointAt(document.createElement("div"));
    expect(destinationReceiverAt(20, 20)).toBeNull();
  });

  it("scrolls only inside the tree's bounded top and bottom edges", () => {
    const rect = { left: 100, right: 300, top: 50, bottom: 250 };

    expect(edgeScrollStep(200, 52, rect)).toBeLessThan(0);
    expect(edgeScrollStep(200, 248, rect)).toBeGreaterThan(0);
    expect(edgeScrollStep(200, 150, rect)).toBe(0);
    expect(edgeScrollStep(50, 52, rect)).toBe(0);
  });

  it("keeps the payload card inside the viewport and away from the pointer", () => {
    expect(destinationDragPreviewPosition(100, 100, 800, 600)).toEqual({
      left: 116,
      top: 116,
    });
    expect(destinationDragPreviewPosition(790, 590, 800, 600)).toEqual({
      left: 554,
      top: 510,
    });
  });
});

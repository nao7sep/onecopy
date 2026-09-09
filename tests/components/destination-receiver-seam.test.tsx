// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useDragDropManager, type DragDropManager } from "@dnd-kit/react";
import { DOMRectangle } from "@dnd-kit/dom/utilities";
import DestinationDragProvider, { useDestinationItemDrag } from "../../src/components/DestinationDragProvider";
import DestinationsTab from "../../src/components/DestinationsTab";
import { useDestinationsStore } from "../../src/state/destinations-store";
import { useItemsStore } from "../../src/state/items-store";
import { beginDestinationDrag, cancelDestinationDrag } from "../../src/workflows/destinations";
import { invokeCalls, mockCommands, resetTauriMocks } from "../mocks/tauri";

let manager: DragDropManager;
function Source() {
  manager = useDragDropManager()!;
  const drag = useDestinationItemDrag({ key: "h1", label: "photo.jpg", thumbHash: null });
  return <div ref={drag.ref} data-testid="source">Photo</div>;
}

beforeEach(() => {
  resetTauriMocks();
  mockCommands({ log_event: () => null });
  useItemsStore.setState({ selectedItem: "h1", selectedKeys: new Set(["h1"]), selectedPositions: new Map([["h1", 0]]) });
  useDestinationsStore.setState({ roots: ["/keep", "/archive"], children: {}, listing: {}, expanded: new Set(), emptiness: {}, activePath: null, dragSelection: null, pendingDrop: null });
});
afterEach(() => { cleanup(); cancelDestinationDrag(); vi.restoreAllMocks(); });

it("uses the registered row's pointer collision, denying shape overlap and clipped/covered pixels", async () => {
  const view = render(<DestinationDragProvider><Source /><DestinationsTab /></DestinationDragProvider>);
  const source = view.getByTestId("source");
  const row = view.container.querySelector<HTMLElement>("[data-tree-path='/keep']")!;
  vi.spyOn(source, "getBoundingClientRect").mockReturnValue(new DOMRect(200, 100, 220, 64));
  vi.spyOn(row, "getBoundingClientRect").mockReturnValue(new DOMRect(400, 100, 180, 40));
  const hit = vi.spyOn(document, "elementFromPoint").mockReturnValue(source);
  const receiver = manager.registry.droppables.get("destination-receiver:/keep")!;
  manager.actions.setDragSource("destination-item:h1");
  receiver.refreshShape();
  manager.dragOperation.shape = new DOMRectangle(source);
  const collide = (x: number, y: number) => {
    manager.dragOperation.position.current = { x, y };
    return receiver.collisionDetector({ droppable: receiver, dragOperation: manager.dragOperation.snapshot() });
  };
  // The thumbnail extends into the row but the pointer remains in Main.
  expect(collide(395, 120)).toBeNull();
  hit.mockReturnValue(row.querySelector("span")!);
  expect(collide(410, 120)?.id).toBe(receiver.id);
  // The row rectangle may extend behind a scroller edge or overlay. Native
  // DOM hit testing, not a second app-owned clipping algorithm, owns visibility.
  hit.mockReturnValue(view.container);
  expect(collide(410, 120)).toBeNull();
  hit.mockReturnValue(null);
  expect(collide(410, 120)).toBeNull();
  row.remove();
  hit.mockReturnValue(row);
  expect(collide(410, 120)).toBeNull();
});

it("routes release through current registered DOM rows even if the last hover target is stale", async () => {
  const view = render(<DestinationDragProvider><Source /><DestinationsTab /></DestinationDragProvider>);
  const keep = view.container.querySelector<HTMLElement>("[data-tree-path='/keep']")!;
  const archive = view.container.querySelector<HTMLElement>("[data-tree-path='/archive']")!;
  const hit = vi.spyOn(document, "elementFromPoint").mockReturnValue(archive);
  manager.actions.setDragSource("destination-item:h1");
  await act(async () => { await manager.actions.setDropTarget("destination-receiver:/keep"); });
  const release = (canceled = false) => act(() => {
    beginDestinationDrag("h1");
    manager.monitor.dispatch("dragend", {
      operation: manager.dragOperation.snapshot(), canceled,
      nativeEvent: new PointerEvent("pointerup", { clientX: 450, clientY: 170 }),
      suspend: () => ({ resume() {}, abort() {} }),
    });
  });
  release();
  expect(useDestinationsStore.getState().pendingDrop?.path).toBe("/archive");
  expect(hit).toHaveBeenCalledWith(450, 170);
  expect(invokeCalls.some(({ command }) => command === "move_items_out")).toBe(false);

  act(() => { useDestinationsStore.setState({ pendingDrop: null }); });
  hit.mockReturnValue(view.container); // final pointer in Main/pane chrome
  release();
  expect(useDestinationsStore.getState().pendingDrop).toBeNull();
  expect(useDestinationsStore.getState().dragSelection).toBeNull();
  hit.mockReturnValue(keep);
  release(true);
  expect(useDestinationsStore.getState().pendingDrop).toBeNull();

  manager.registry.droppables.get("destination-receiver:/keep")!.disabled = true;
  release();
  expect(useDestinationsStore.getState().pendingDrop).toBeNull();
});

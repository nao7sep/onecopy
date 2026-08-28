// @vitest-environment happy-dom

import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EMPTY_ITEM_WORK, type SectionItem } from "../../src/models/items";
import { useDestinationsStore } from "../../src/state/destinations-store";
import { useItemsStore } from "../../src/state/items-store";
import { cancelDestinationDrag } from "../../src/workflows/destinations";
import { mockCommands, resetTauriMocks } from "../mocks/tauri";

const dnd = vi.hoisted(() => ({
  provider: null as Record<string, (...args: any[]) => void> | null,
  draggables: [] as Record<string, unknown>[],
  droppables: [] as Record<string, unknown>[],
  activeDropId: null as string | null,
}));

vi.mock("@dnd-kit/react", async () => {
  const ReactModule = await import("react");
  return {
    DragDropProvider: ({ children, ...props }: any) => {
      dnd.provider = props;
      return ReactModule.createElement(ReactModule.Fragment, null, children);
    },
    DragOverlay: () => null,
    useDraggable: (input: Record<string, unknown>) => {
      dnd.draggables.push(input);
      return { ref: () => undefined, isDragging: false };
    },
    useDroppable: (input: Record<string, unknown>) => {
      dnd.droppables.push(input);
      return {
        ref: () => undefined,
        isDropTarget: input.id === dnd.activeDropId,
      };
    },
  };
});

import DestinationDragProvider, {
  useDestinationItemDrag,
  useDestinationReceiver,
} from "../../src/components/DestinationDragProvider";
import DestinationDragPreview from "../../src/components/DestinationDragPreview";
import DestinationsTab from "../../src/components/DestinationsTab";

function item(
  hash: string,
  fileName: string,
  pathId: number,
): SectionItem {
  return {
    hash,
    pathId,
    fileName,
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

const source = {
  key: "h1",
  label: "portrait.png",
  thumbHash: null,
};

function RegistrationHarness() {
  const draggable = useDestinationItemDrag(source);
  const receiver = useDestinationReceiver("/keep");
  return (
    <>
      <div ref={draggable.ref}>Source</div>
      <div ref={receiver.ref}>Keep</div>
    </>
  );
}

beforeEach(() => {
  resetTauriMocks({ keepListeners: true });
  mockCommands({ log_event: () => null });
  dnd.provider = null;
  dnd.draggables = [];
  dnd.droppables = [];
  dnd.activeDropId = null;
  useDestinationsStore.setState({
    roots: [],
    children: {},
    listing: {},
    expanded: new Set(),
    emptiness: {},
    activePath: null,
    dragSelection: null,
    pendingDrop: null,
  });
  useItemsStore.setState({
    selected: { kind: "image", month: "2026-08" },
    items: [item("h1", "portrait.png", 1)],
    selectedItem: "h1",
    selectedKeys: new Set(["h1"]),
    rangeOrigin: "h1",
    rangeBase: new Set(["h1"]),
  });
});

afterEach(() => {
  cleanup();
  cancelDestinationDrag();
});

describe("destination drag transport", () => {
  it("registers only indexed items and semantic destination rows", () => {
    render(
      <DestinationDragProvider>
        <RegistrationHarness />
      </DestinationDragProvider>,
    );

    expect(dnd.draggables.at(-1)).toMatchObject({
      id: "destination-item:h1",
      type: "destination-item",
      data: source,
    });
    expect(dnd.droppables.at(-1)).toMatchObject({
      id: "destination-receiver:/keep",
      accept: "destination-item",
      data: { path: "/keep" },
    });
  });

  it("freezes selection at activation and gives a valid release to the workflow", () => {
    render(
      <DestinationDragProvider>
        <RegistrationHarness />
      </DestinationDragProvider>,
    );
    const provider = dnd.provider!;
    const preventDefault = vi.fn();

    act(() => {
      provider.onBeforeDragStart({
        operation: { source: { data: source } },
        preventDefault,
      });
    });
    expect(preventDefault).not.toHaveBeenCalled();
    expect(useDestinationsStore.getState().dragSelection).toMatchObject({
      items: [{ hash: "h1", pathId: null }],
    });

    act(() => {
      provider.onDragEnd({
        canceled: false,
        nativeEvent: new PointerEvent("pointerup"),
        operation: { target: { data: { path: "/keep" } } },
      });
    });
    expect(useDestinationsStore.getState().pendingDrop).toMatchObject({
      path: "/keep",
      selection: { items: [{ hash: "h1", pathId: null }] },
    });
    expect(useDestinationsStore.getState().dragSelection).toBeNull();
  });

  it("cancels silently when release has no semantic receiver", () => {
    render(<DestinationDragProvider>Content</DestinationDragProvider>);
    const provider = dnd.provider!;
    act(() => {
      provider.onBeforeDragStart({
        operation: { source: { data: source } },
        preventDefault: vi.fn(),
      });
      provider.onDragEnd({
        canceled: false,
        operation: { target: null },
      });
    });

    expect(useDestinationsStore.getState().dragSelection).toBeNull();
    expect(useDestinationsStore.getState().pendingDrop).toBeNull();
  });

  it("shows the library-selected receiver and a truthful multi-item preview", () => {
    dnd.activeDropId = "destination-receiver:/keep";
    useDestinationsStore.setState({
      roots: ["/archive", "/keep"],
      dragSelection: {
        items: [
          { hash: "h1", pathId: null },
          { hash: "h2", pathId: null },
        ],
        blockedNameCount: 0,
        anchorKey: "h1",
        shownKeys: ["h1", "h2"],
      },
    });

    const view = render(
      <>
        <DestinationsTab />
        <DestinationDragPreview source={source} />
      </>,
    );
    const archive = view.container.querySelector<HTMLElement>(
      "[data-tree-path='/archive']",
    )!;
    const keep = view.container.querySelector<HTMLElement>(
      "[data-tree-path='/keep']",
    )!;

    expect(archive.className).not.toContain("ring-2");
    expect(keep.className).toContain("ring-2");
    expect(view.getByText("2 selected items")).toBeTruthy();
    expect(view.getByText("Includes portrait.png")).toBeTruthy();
    expect(
      view.getByText("2 selected items").parentElement?.textContent,
    ).not.toMatch(/move|copy/i);
  });
});

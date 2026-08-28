import type { ReactNode } from "react";
import {
  DragDropProvider,
  DragOverlay,
  useDraggable,
  useDroppable,
} from "@dnd-kit/react";
import {
  Accessibility,
  PointerActivationConstraints,
  PointerSensor,
} from "@dnd-kit/dom";
import { useDestinationsStore } from "../state/destinations-store";
import {
  beginDestinationDrag,
  cancelDestinationDrag,
  takeDestinationDrag,
} from "../workflows/destinations";
import DestinationDragPreview from "./DestinationDragPreview";

const ITEM_TYPE = "destination-item";
const ITEM_ID_PREFIX = "destination-item:";
const RECEIVER_ID_PREFIX = "destination-receiver:";

// Each draggable is also the listbox's click-to-select surface. Immediate
// pointer activation would turn an ordinary selection click into drag start;
// this maintained sensor constraint preserves the two product gestures.
const POINTER_SENSOR = PointerSensor.configure({
  activationConstraints: [
    new PointerActivationConstraints.Distance({ value: 6 }),
  ],
});

export interface DestinationDragSource {
  key: string;
  label: string;
  thumbHash: string | null;
}

/**
 * Owns the transport for OneCopy's internal item-to-destination gesture.
 * Product authority stays in workflows/destinations: this component only
 * turns library drag events into begin, cancel, and receive decisions.
 */
export default function DestinationDragProvider({
  children,
}: {
  children: ReactNode;
}) {
  return (
    <DragDropProvider
      // Replacing the defaults deliberately removes dnd-kit's KeyboardSensor.
      // The virtualized grid is one active-descendant listbox, so each item
      // must not become another tab stop; the destination action bar and its
      // Enter/Cmd-or-Ctrl+Enter commands are the equivalent keyboard path.
      sensors={[POINTER_SENSOR]}
      // Accessibility would add button semantics, tabindex, and keyboard-drag
      // instructions to every item. Those promises conflict with the same
      // single-tab-stop listbox, so retain every default plugin except it.
      plugins={(defaults) =>
        defaults.filter((plugin) => plugin !== Accessibility)
      }
      onBeforeDragStart={(event) => {
        const source = event.operation.source?.data as
          | DestinationDragSource
          | undefined;
        if (source === undefined || beginDestinationDrag(source.key) === null) {
          event.preventDefault();
        }
      }}
      onDragEnd={(event) => {
        const path = event.operation.target?.data.path as string | undefined;
        if (event.canceled || path === undefined) {
          cancelDestinationDrag();
          return;
        }

        const selection = takeDestinationDrag();
        if (selection === null) return;
        useDestinationsStore.getState().setActive(path);
        useDestinationsStore.getState().setPendingDrop({
          path,
          selection,
        });
      }}
    >
      {children}
      {/* The default return-to-source animation would imply rejection after
          this receiver has opened the accepted Move/Copy choice. */}
      <DragOverlay dropAnimation={null}>
        {(source) => (
          <DestinationDragPreview
            source={source.data as DestinationDragSource}
          />
        )}
      </DragOverlay>
    </DragDropProvider>
  );
}

export function useDestinationItemDrag(source: DestinationDragSource) {
  const { ref, isDragging } = useDraggable({
    id: `${ITEM_ID_PREFIX}${source.key}`,
    type: ITEM_TYPE,
    data: source,
  });
  return { ref, isDragging };
}

export function useDestinationReceiver(path: string) {
  const { ref, isDropTarget } = useDroppable({
    id: `${RECEIVER_ID_PREFIX}${path}`,
    accept: ITEM_TYPE,
    data: { path },
  });
  return { ref, isDropTarget };
}

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
  moveDestinationSelectionTo,
  takeDestinationDrag,
} from "../workflows/destinations";
import DestinationDragPreview from "./DestinationDragPreview";

const ITEM_TYPE = "destination-item";
const ITEM_ID_PREFIX = "destination-item:";
const RECEIVER_ID_PREFIX = "destination-receiver:";

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
      sensors={[POINTER_SENSOR]}
      // Drag is supplementary to the destination action bar. Keeping every
      // grid item inside the existing listbox keyboard model is more usable
      // than dnd-kit's per-item keyboard drag affordance here.
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

        const nativeEvent = event.nativeEvent as
          | Pick<MouseEvent, "ctrlKey" | "metaKey">
          | undefined;
        if (nativeEvent?.metaKey || nativeEvent?.ctrlKey) {
          void moveDestinationSelectionTo(path, "copy", selection);
        } else {
          useDestinationsStore.getState().setPendingDrop({
            path,
            selection,
          });
        }
      }}
    >
      {children}
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

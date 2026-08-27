import { useEffect, useRef, useState } from "react";
import { useDestinationsStore } from "../state/destinations-store";
import {
  beginDestinationDrag,
  cancelDestinationDrag,
  moveDestinationSelectionTo,
  takeDestinationDrag,
} from "../workflows/destinations";

const DRAG_THRESHOLD_PX = 6;
const EDGE_MARGIN_PX = 32;
const MAX_EDGE_STEP_PX = 10;

interface ActivePointerDrag {
  pointerId: number;
  startX: number;
  startY: number;
  lastX: number;
  lastY: number;
  dragging: boolean;
  itemCount: number;
  frame: number | null;
}

interface DestinationDragSource {
  key: string;
  label: string;
  thumbHash: string | null;
}

export function destinationReceiverAt(
  clientX: number,
  clientY: number,
): string | null {
  const receiver = document
    .elementFromPoint(clientX, clientY)
    ?.closest<HTMLElement>("[data-destination-receiver]");
  return receiver?.dataset.destinationReceiver ?? null;
}

export function edgeScrollStep(
  clientX: number,
  clientY: number,
  rect: Pick<DOMRect, "left" | "right" | "top" | "bottom">,
): number {
  if (
    clientX < rect.left ||
    clientX > rect.right ||
    clientY < rect.top ||
    clientY > rect.bottom
  ) return 0;
  if (clientY < rect.top + EDGE_MARGIN_PX) {
    const pressure = (rect.top + EDGE_MARGIN_PX - clientY) / EDGE_MARGIN_PX;
    return -Math.max(1, Math.ceil(pressure * MAX_EDGE_STEP_PX));
  }
  if (clientY > rect.bottom - EDGE_MARGIN_PX) {
    const pressure = (clientY - (rect.bottom - EDGE_MARGIN_PX)) / EDGE_MARGIN_PX;
    return Math.max(1, Math.ceil(pressure * MAX_EDGE_STEP_PX));
  }
  return 0;
}

/**
 * Owns OneCopy's same-window Move/Copy gesture.
 *
 * Native HTML drag is intentionally not used here: WKWebView can start and
 * end that transport without delivering dragover/drop to a sibling pane.
 * Pointer events keep the interaction inside the app, while the frozen
 * workflow selection remains the authority for the eventual operation.
 */
export function useDestinationPointerDrag(source: DestinationDragSource) {
  const active = useRef<ActivePointerDrag | null>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const suppressClick = useRef(false);
  const [dragging, setDragging] = useState(false);

  useEffect(
    () => () => {
      cleanupRef.current?.();
    },
    [],
  );

  const onPointerDown = (event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0 || !event.isPrimary) return;
    cleanupRef.current?.();

    const gesture: ActivePointerDrag = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      dragging: false,
      itemCount: 0,
      frame: null,
    };
    active.current = gesture;

    const stopFrame = () => {
      if (gesture.frame !== null) cancelAnimationFrame(gesture.frame);
      gesture.frame = null;
    };
    const updateReceiver = () => {
      useDestinationsStore.setState({
        dragReceiverPath: destinationReceiverAt(
          gesture.lastX,
          gesture.lastY,
        ),
        dragPresentation: {
          clientX: gesture.lastX,
          clientY: gesture.lastY,
          sourceLabel: source.label,
          itemCount: gesture.itemCount,
          thumbHash: source.thumbHash,
        },
      });
    };
    const runEdgeScroll = () => {
      gesture.frame = null;
      if (!gesture.dragging || active.current !== gesture) return;
      const scroller = document.querySelector<HTMLElement>(
        "[data-destination-scroll]",
      );
      if (scroller === null) return;
      const step = edgeScrollStep(
        gesture.lastX,
        gesture.lastY,
        scroller.getBoundingClientRect(),
      );
      if (step === 0) return;
      scroller.scrollTop += step;
      updateReceiver();
      gesture.frame = requestAnimationFrame(runEdgeScroll);
    };
    const scheduleEdgeScroll = () => {
      if (gesture.frame === null) {
        gesture.frame = requestAnimationFrame(runEdgeScroll);
      }
    };
    const removeListeners = () => {
      document.removeEventListener("pointermove", onPointerMove);
      document.removeEventListener("pointerup", onPointerUp);
      document.removeEventListener("pointercancel", onPointerCancel);
      window.removeEventListener("blur", onCancel);
      window.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      stopFrame();
      cleanupRef.current = null;
      if (active.current === gesture) active.current = null;
      setDragging(false);
    };
    const releaseClickSuppression = () => {
      setTimeout(() => {
        suppressClick.current = false;
      }, 0);
    };
    const cancel = () => {
      const wasDragging = gesture.dragging;
      removeListeners();
      if (wasDragging) {
        cancelDestinationDrag();
        releaseClickSuppression();
      }
    };
    const onCancel = () => cancel();
    const onKeyDown = (keyEvent: KeyboardEvent) => {
      if (keyEvent.key !== "Escape") return;
      if (gesture.dragging) keyEvent.preventDefault();
      cancel();
    };
    const onVisibilityChange = () => {
      if (document.hidden) cancel();
    };
    const onPointerCancel = (pointerEvent: PointerEvent) => {
      if (pointerEvent.pointerId === gesture.pointerId) cancel();
    };
    const onPointerMove = (pointerEvent: PointerEvent) => {
      if (pointerEvent.pointerId !== gesture.pointerId) return;
      gesture.lastX = pointerEvent.clientX;
      gesture.lastY = pointerEvent.clientY;
      if (!gesture.dragging) {
        const distance = Math.hypot(
          gesture.lastX - gesture.startX,
          gesture.lastY - gesture.startY,
        );
        if (distance < DRAG_THRESHOLD_PX) return;
        const selection = beginDestinationDrag(source.key);
        if (selection === null) {
          removeListeners();
          return;
        }
        gesture.dragging = true;
        gesture.itemCount = selection.items.length;
        suppressClick.current = true;
        setDragging(true);
      }
      pointerEvent.preventDefault();
      updateReceiver();
      scheduleEdgeScroll();
    };
    const onPointerUp = (pointerEvent: PointerEvent) => {
      if (pointerEvent.pointerId !== gesture.pointerId) return;
      const wasDragging = gesture.dragging;
      gesture.lastX = pointerEvent.clientX;
      gesture.lastY = pointerEvent.clientY;
      const path = wasDragging
        ? destinationReceiverAt(gesture.lastX, gesture.lastY)
        : null;
      removeListeners();
      if (!wasDragging) return;

      pointerEvent.preventDefault();
      const selection = path === null ? null : takeDestinationDrag();
      if (path === null || selection === null) {
        cancelDestinationDrag();
      } else {
        useDestinationsStore.getState().setActive(path);
        if (pointerEvent.metaKey || pointerEvent.ctrlKey) {
          void moveDestinationSelectionTo(path, "copy", selection);
        } else {
          useDestinationsStore.getState().setPendingDrop({ path, selection });
        }
      }
      // Browsers dispatch click immediately after pointerup. Clear on the next
      // task so that release cannot also toggle the source selection.
      releaseClickSuppression();
    };

    cleanupRef.current = cancel;
    document.addEventListener("pointermove", onPointerMove, { passive: false });
    document.addEventListener("pointerup", onPointerUp, { passive: false });
    document.addEventListener("pointercancel", onPointerCancel);
    window.addEventListener("blur", onCancel);
    window.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("visibilitychange", onVisibilityChange);
  };

  const onClickCapture = (event: React.MouseEvent<HTMLElement>) => {
    if (!suppressClick.current) return;
    event.preventDefault();
    event.stopPropagation();
  };

  return { dragging, handlers: { onPointerDown, onClickCapture } };
}

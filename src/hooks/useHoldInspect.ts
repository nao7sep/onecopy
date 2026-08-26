import {
  useEffect,
  useRef,
  useState,
  type MouseEventHandler,
  type PointerEventHandler,
} from "react";

export interface PointerPoint {
  clientX: number;
  clientY: number;
}

interface HoldInspectOptions {
  onStart: (point: PointerPoint) => void;
  onMove: (point: PointerPoint) => void;
  onEnd: () => void;
  delayMs?: number;
}

interface PendingHold {
  pointerId: number;
  owner: HTMLElement;
  point: PointerPoint;
  timer: number;
}

/**
 * One momentary press-and-hold recognizer for full-resolution media inspection.
 * Pointer capture begins only after the hold wins, so ordinary clicks and
 * native video controls keep their normal pointer sequence.
 */
export function useHoldInspect({
  onStart,
  onMove,
  onEnd,
  delayMs = 135,
}: HoldInspectOptions): {
  inspecting: boolean;
  onPointerDown: PointerEventHandler<HTMLElement>;
  onClickCapture: MouseEventHandler<HTMLElement>;
} {
  const callbacksRef = useRef({ onStart, onMove, onEnd });
  callbacksRef.current = { onStart, onMove, onEnd };
  const pendingRef = useRef<PendingHold | null>(null);
  const inspectingRef = useRef(false);
  const suppressClickRef = useRef(false);
  const [inspecting, setInspecting] = useState(false);

  useEffect(() => {
    const finish = (pointerId: number | null, suppressClick: boolean) => {
      const pending = pendingRef.current;
      if (pending === null || (pointerId !== null && pointerId !== pending.pointerId)) return;
      window.clearTimeout(pending.timer);
      pendingRef.current = null;
      if (inspectingRef.current) {
        inspectingRef.current = false;
        suppressClickRef.current = suppressClick;
        callbacksRef.current.onEnd();
        setInspecting(false);
      }
      if (pending.owner.hasPointerCapture?.(pending.pointerId)) {
        pending.owner.releasePointerCapture(pending.pointerId);
      }
    };

    const move = (event: PointerEvent) => {
      const pending = pendingRef.current;
      if (pending === null || pending.pointerId !== event.pointerId) return;
      pending.point = { clientX: event.clientX, clientY: event.clientY };
      if (inspectingRef.current) callbacksRef.current.onMove(pending.point);
    };
    const up = (event: PointerEvent) => finish(event.pointerId, true);
    const cancel = (event: PointerEvent) => finish(event.pointerId, true);
    const lostCapture = (event: PointerEvent) => finish(event.pointerId, true);
    const blur = () => finish(null, true);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("lostpointercapture", lostCapture);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("lostpointercapture", lostCapture);
      window.removeEventListener("blur", blur);
      const pending = pendingRef.current;
      if (pending !== null) window.clearTimeout(pending.timer);
      pendingRef.current = null;
      if (inspectingRef.current) callbacksRef.current.onEnd();
      inspectingRef.current = false;
    };
  }, []);

  const onPointerDown: PointerEventHandler<HTMLElement> = (event) => {
    if (event.button !== 0 || event.isPrimary === false) return;
    if ((event.target as Element).closest("button, a, input, select, textarea")) return;
    if (pendingRef.current !== null) return;
    // If the previous hold ended somewhere that produced no click, this is a
    // genuinely new gesture and must not inherit its suppression token.
    suppressClickRef.current = false;
    const pending: PendingHold = {
      pointerId: event.pointerId,
      owner: event.currentTarget,
      point: { clientX: event.clientX, clientY: event.clientY },
      timer: 0,
    };
    pending.timer = window.setTimeout(() => {
      if (pendingRef.current !== pending) return;
      inspectingRef.current = true;
      setInspecting(true);
      pending.owner.setPointerCapture?.(pending.pointerId);
      callbacksRef.current.onStart(pending.point);
    }, delayMs);
    pendingRef.current = pending;
  };

  const onClickCapture: MouseEventHandler<HTMLElement> = (event) => {
    if (!suppressClickRef.current) return;
    suppressClickRef.current = false;
    event.preventDefault();
    event.stopPropagation();
  };

  return { inspecting, onPointerDown, onClickCapture };
}

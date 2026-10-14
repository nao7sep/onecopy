// The whole of a shortened value, shown on request (interface-styling-
// conventions, "Identifying text is shortened": the complete value is
// reachable by keyboard and touch, never by a hover tooltip alone).
//
// A small surface anchored to the control that opened it. It holds focus and
// joins the modal stack through the shared layer, so Escape closes it alone —
// over a dialog too — and the window's own command keys stay quiet while it is
// open. The text is selectable and scrolls inside a height limit, so a long
// path or list never grows the popover past the viewport. A pointer press
// outside closes it; the opener toggles it.

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useModalLayer } from "../../hooks/useModalLayer";

const VIEWPORT_GAP = 8;
const stop = (event: { stopPropagation: () => void }) => event.stopPropagation();
const ANCHOR_GAP = 4;

export default function RevealPopover({
  anchor,
  labelledBy,
  onClose,
  children,
}: {
  /** The control that opened the popover, which positions it. */
  anchor: HTMLElement;
  /** The opener's id: its text names what the popover shows in full. */
  labelledBy: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const surfaceRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ top: number; left: number }>();
  useModalLayer(surfaceRef, onClose);

  useLayoutEffect(() => {
    const surface = surfaceRef.current;
    if (surface === null) return;
    const anchorRect = anchor.getBoundingClientRect();
    const own = surface.getBoundingClientRect();
    const below = anchorRect.bottom + ANCHOR_GAP;
    const top = below + own.height <= window.innerHeight - VIEWPORT_GAP
      ? below
      : Math.max(VIEWPORT_GAP, anchorRect.top - ANCHOR_GAP - own.height);
    const left = Math.min(
      Math.max(anchorRect.left, VIEWPORT_GAP),
      Math.max(VIEWPORT_GAP, window.innerWidth - VIEWPORT_GAP - own.width),
    );
    setPosition({ top, left });
  }, [anchor]);

  useEffect(() => {
    const onOutside = (event: MouseEvent) => {
      const target = event.target as Node;
      // The opener toggles the popover on its own click.
      if (surfaceRef.current?.contains(target) || anchor.contains(target)) return;
      onClose();
    };
    document.addEventListener("mousedown", onOutside);
    return () => document.removeEventListener("mousedown", onOutside);
  }, [anchor, onClose]);

  return createPortal(
    <div
      ref={surfaceRef}
      role="dialog"
      aria-labelledby={labelledBy}
      tabIndex={-1}
      data-reveal-popover
      // Transparent rather than hidden until placed: the shared layer focuses
      // the surface in the same commit, and a hidden element takes no focus.
      style={{ ...position, opacity: position === undefined ? 0 : 1 }}
      // Portalled in the DOM but not in React's tree: a press or key here must
      // not reach the row, card or list the opener sits in.
      onClick={stop}
      onDoubleClick={stop}
      onMouseDown={stop}
      onKeyDown={stop}
      className="fixed z-50 max-h-40 w-max max-w-[min(28rem,calc(100vw-1rem))] select-text overflow-y-auto rounded-lg border border-border bg-surface px-3 py-2 text-left text-sm text-ink shadow-xl outline-none"
    >
      {children}
    </div>,
    document.body,
  );
}

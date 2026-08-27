import { createPortal } from "react-dom";
import { File } from "lucide-react";
import { thumbUrl } from "../models/items";
import { useDestinationsStore } from "../state/destinations-store";

const PREVIEW_WIDTH = 220;
const PREVIEW_HEIGHT = 64;
const POINTER_GAP = 16;
const VIEWPORT_GAP = 8;

export function destinationDragPreviewPosition(
  clientX: number,
  clientY: number,
  viewportWidth: number,
  viewportHeight: number,
): { left: number; top: number } {
  const right = clientX + POINTER_GAP;
  const left = clientX - POINTER_GAP - PREVIEW_WIDTH;
  const below = clientY + POINTER_GAP;
  const above = clientY - POINTER_GAP - PREVIEW_HEIGHT;
  return {
    left:
      right + PREVIEW_WIDTH <= viewportWidth - VIEWPORT_GAP
        ? right
        : Math.max(VIEWPORT_GAP, left),
    top:
      below + PREVIEW_HEIGHT <= viewportHeight - VIEWPORT_GAP
        ? below
        : Math.max(VIEWPORT_GAP, above),
  };
}

/** Visual payload only. Pointer events pass through it, so semantic receiver
 * hit-testing always sees the real row underneath. */
export default function DestinationDragPreview() {
  const presentation = useDestinationsStore((state) => state.dragPresentation);
  if (presentation === null) return null;
  const position = destinationDragPreviewPosition(
    presentation.clientX,
    presentation.clientY,
    window.innerWidth,
    window.innerHeight,
  );
  const multiple = presentation.itemCount > 1;

  return createPortal(
    <div
      aria-hidden="true"
      className="pointer-events-none fixed z-[100] flex h-16 w-[220px] select-none items-center gap-2.5 rounded-lg border-2 border-primary-ring bg-surface px-2.5 shadow-xl"
      style={position}
    >
      <span className="flex h-11 w-11 shrink-0 items-center justify-center overflow-hidden rounded-md border border-border bg-background text-ink-muted">
        {presentation.thumbHash !== null ? (
          <img
            src={thumbUrl(presentation.thumbHash)}
            alt=""
            draggable={false}
            className="h-full w-full object-contain"
          />
        ) : (
          <File size={20} aria-hidden="true" />
        )}
      </span>
      <span className="min-w-0">
        <span className="block truncate text-sm font-semibold text-ink-strong">
          {multiple
            ? `${presentation.itemCount} selected items`
            : presentation.sourceLabel}
        </span>
        <span className="block truncate text-xs text-ink-muted">
          {multiple ? `Includes ${presentation.sourceLabel}` : "Selected item"}
        </span>
      </span>
    </div>,
    document.body,
  );
}

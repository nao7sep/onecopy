import { File } from "lucide-react";
import { thumbUrl } from "../models/items";
import { useDestinationsStore } from "../state/destinations-store";
import type { DestinationDragSource } from "./DestinationDragProvider";

/** Visual payload only; dnd-kit owns its position and pointer transparency. */
export default function DestinationDragPreview({
  source,
}: {
  source: DestinationDragSource;
}) {
  const itemCount = useDestinationsStore(
    (state) => state.dragSelection?.items.length ?? 0,
  );
  const multiple = itemCount > 1;

  return (
    <div
      aria-hidden="true"
      className="pointer-events-none flex h-16 w-[220px] select-none items-center gap-2.5 rounded-lg border-2 border-primary-ring bg-surface px-2.5 shadow-xl"
    >
      <span className="flex h-11 w-11 shrink-0 items-center justify-center overflow-hidden rounded-md border border-border bg-background text-ink-muted">
        {source.thumbHash !== null ? (
          <img
            src={thumbUrl(source.thumbHash)}
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
            ? `${itemCount} selected items`
            : source.label}
        </span>
        <span className="block truncate text-xs text-ink-muted">
          {multiple ? `Includes ${source.label}` : "Selected item"}
        </span>
      </span>
    </div>
  );
}

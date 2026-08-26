import { useEffect, useRef } from "react";
import { X } from "lucide-react";
import { itemKey, useItemsStore } from "../state/items-store";
import { useQuickViewStore } from "../state/quick-view-store";
import { useModalLayer } from "../hooks/useModalLayer";
import PreviewSurface from "./PreviewSurface";

export default function QuickView() {
  const close = useQuickViewStore((state) => state.close);
  const selectedItem = useItemsStore((state) => state.selectedItem);
  const items = useItemsStore((state) => state.items);
  const detail = useItemsStore((state) => state.detail);
  const surfaceRef = useRef<HTMLDivElement>(null);
  useModalLayer(surfaceRef, close);
  const item = items.find((candidate) => itemKey(candidate) === selectedItem) ?? null;

  useEffect(() => {
    if (item === null) close();
  }, [close, item]);

  if (item === null) return null;
  return (
    <div
      ref={surfaceRef}
      tabIndex={-1}
      className="fixed inset-0 z-30 flex flex-col bg-background outline-none"
      role="dialog"
      aria-modal="true"
      aria-label="Quick View"
    >
      <div className="flex shrink-0 items-center justify-between border-b border-border bg-surface px-3 py-2">
        <span className="min-w-0 truncate text-sm text-ink" title={item.fileName}>
          {item.fileName}
        </span>
        <button
          data-modal-close
          aria-label="Close Quick View"
          className="flex h-8 w-8 items-center justify-center rounded-md text-ink-muted hover:bg-surface-muted hover:text-ink"
          onClick={close}
        >
          <X size={16} />
        </button>
      </div>
      <div className="min-h-0 flex-1">
        {detail === null ? (
          <div className="flex h-full items-center justify-center text-sm text-ink-muted">
            Loading…
          </div>
        ) : (
          <PreviewSurface
            hash={item.hash}
            pathId={item.hash === null ? item.pathId : null}
            detail={detail}
            keyboardActive
            autoplayImmediately
          />
        )}
      </div>
      <footer className="shrink-0 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        Escape: back to the grid
      </footer>
    </div>
  );
}

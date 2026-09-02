import { useEffect, useRef } from "react";
import { ChevronLeft, ChevronRight, Maximize2, X } from "lucide-react";
import { useModalLayer } from "../hooks/useModalLayer";
import { useItemsStore } from "../state/items-store";
import { useQuickViewStore } from "../state/quick-view-store";
import {
  closeViewer,
  confirmViewerDelete,
  handleViewerKey,
  moveViewer,
  setViewerPresentation,
} from "../workflows/quick-view";
import ConfirmDialog from "./ConfirmDialog";
import PreviewSurface from "./PreviewSurface";
import { identityKey, isAudioFile } from "../models/items";
import OperationResult from "./ui/OperationResult";

export default function QuickView() {
  const session = useQuickViewStore((state) => state.session);
  const pendingDelete = useQuickViewStore((state) => state.pendingDelete);
  const failure = useQuickViewStore((state) => state.failure);
  const selectedItem = useItemsStore((state) => state.selectedItem);
  const detail = useItemsStore((state) => state.detail);
  const sectionKind = useItemsStore((state) => state.selected?.kind ?? null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  useModalLayer(surfaceRef, () => void closeViewer());

  const quickOpen = session?.presentation === "quick";
  const key = quickOpen && session !== null ? identityKey(session.member) : null;
  const item = quickOpen ? (session?.item ?? null) : null;

  useEffect(() => {
    if (!quickOpen || item === null) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (pendingDelete !== null) return;
      const notificationControl =
        event.target instanceof Element && event.target.closest("[data-notification]") !== null;
      if (notificationControl && event.key === "Enter") return;
      const handled = new Set([
        " ",
        "f",
        "F",
        "ArrowLeft",
        "ArrowRight",
        "Delete",
        "Backspace",
      ]).has(event.key) ||
        (sectionKind !== "other" && ["PageUp", "PageDown", "Home", "End"].includes(event.key));
      const mediaEnter =
        event.key === "Enter" &&
        (sectionKind === "video" || isAudioFile(item.fileName));
      if (!handled && !mediaEnter) return;
      event.preventDefault();
      event.stopPropagation();
      void handleViewerKey(event);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [item, pendingDelete, quickOpen, sectionKind]);

  if (!quickOpen || session === null || item === null) return null;
  const currentDetail = selectedItem === key ? detail : null;
  const atStart = session.index === 0;
  const atEnd = session.index === session.length - 1;

  return (
    <div
      ref={surfaceRef}
      tabIndex={-1}
      className="fixed inset-0 z-20 flex flex-col bg-background outline-none"
      role="dialog"
      aria-modal="true"
      aria-label="Quick View"
    >
      <header className="flex shrink-0 items-center gap-2 border-b border-border bg-surface px-3 py-2">
        <span className="min-w-0 flex-1 truncate text-sm text-ink" title={item.fileName}>
          {item.fileName}
        </span>
        <span className="text-xs tabular-nums text-ink-muted">
          {session.index + 1} / {session.length}
        </span>
        <button
          aria-label="Previous item"
          disabled={atStart}
          className="flex h-8 w-8 items-center justify-center rounded-md text-ink-muted hover:bg-surface-muted hover:text-ink disabled:opacity-30"
          onClick={() => moveViewer("previous")}
        >
          <ChevronLeft size={16} />
        </button>
        <button
          aria-label="Next item"
          disabled={atEnd}
          className="flex h-8 w-8 items-center justify-center rounded-md text-ink-muted hover:bg-surface-muted hover:text-ink disabled:opacity-30"
          onClick={() => moveViewer("next")}
        >
          <ChevronRight size={16} />
        </button>
        <button
          aria-label="Open full screen"
          className="flex h-8 w-8 items-center justify-center rounded-md text-ink-muted hover:bg-surface-muted hover:text-ink"
          onClick={() => void setViewerPresentation("fullscreen")}
        >
          <Maximize2 size={16} />
        </button>
        <button
          data-modal-close
          aria-label="Close Quick View"
          className="flex h-8 w-8 items-center justify-center rounded-md text-ink-muted hover:bg-surface-muted hover:text-ink"
          onClick={() => void closeViewer()}
        >
          <X size={16} />
        </button>
      </header>
      {failure !== null ? (
        <OperationResult
          level="error"
          className="mx-3 mt-2 shrink-0"
          onDismiss={() => useQuickViewStore.getState().setFailure(null)}
          dismissLabel="Dismiss Quick View result"
        >
          {failure}
        </OperationResult>
      ) : null}
      <div className="min-h-0 flex-1">
        {currentDetail === null ? (
          <div className="flex h-full items-center justify-center text-sm text-ink-muted">
            Loading…
          </div>
        ) : (
          <PreviewSurface
            surface="quick"
            hash={item.hash}
            pathId={item.hash === null ? item.pathId : null}
            detail={currentDetail}
            keyboardActive
          />
        )}
      </div>
      <footer className="shrink-0 border-t border-border bg-surface px-3 py-1 text-xs text-ink-muted">
        Left/Right: navigate · F: full screen · Space or Escape: back to the grid
      </footer>
      {pendingDelete !== null ? (
        <ConfirmDialog
          title={pendingDelete === "permanent" ? "Delete permanently?" : "Move to trash?"}
          message={`${pendingDelete === "permanent" ? "Permanently delete" : "Move"} ${item.fileName}${
            pendingDelete === "permanent"
              ? " and every copy? This cannot be undone."
              : " and every copy to the trash?"
          }`}
          confirmLabel={pendingDelete === "permanent" ? "Delete permanently" : "Move to trash"}
          onConfirm={() => void confirmViewerDelete()}
          onCancel={() => useQuickViewStore.getState().cancelDelete()}
        />
      ) : null}
    </div>
  );
}

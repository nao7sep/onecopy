// Private geometry state for the main shell's adjustable panes. Persisted
// values are intent; rendered widths are clamped against the live container.

import { useCallback, useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { useAppStore } from "../state/app-store";
import {
  PREVIEW_PANE_DEFAULT_WIDTH,
  PREVIEW_PANE_MIN_WIDTH,
  RIGHT_PANE_DEFAULT_WIDTH,
  RIGHT_PANE_MIN_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  clampPaneWidths,
  computeMinWindowWidth,
} from "../utils/windowSizing";

interface PaneIntents {
  left: number;
  right: number;
  preview: number;
}

export function usePaneLayout(splitOpen: boolean) {
  const [paneIntents, setPaneIntents] = useState<PaneIntents>({
    left: SIDEBAR_DEFAULT_WIDTH,
    right: RIGHT_PANE_DEFAULT_WIDTH,
    preview: PREVIEW_PANE_DEFAULT_WIDTH,
  });
  const paneIntentsRef = useRef(paneIntents);
  paneIntentsRef.current = paneIntents;

  const [containerWidth, setContainerWidth] = useState<number | null>(null);
  const contentRowRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const row = contentRowRef.current;
    if (row === null) return;
    const measure = () => setContainerWidth(row.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(row);
    return () => observer.disconnect();
  }, []);

  const paneWidths = clampPaneWidths(
    paneIntents.left,
    paneIntents.right,
    containerWidth ?? computeMinWindowWidth() * 4,
    splitOpen ? paneIntents.preview : null,
  );

  const activeDragCleanup = useRef<(() => void) | null>(null);
  useEffect(
    () => () => {
      activeDragCleanup.current?.();
    },
    [],
  );

  const beginPaneDrag = useCallback(
    (side: "left" | "right" | "preview") => (event: ReactMouseEvent) => {
      event.preventDefault();
      activeDragCleanup.current?.();
      const startX = event.clientX;
      const start = { ...paneIntentsRef.current };
      document.body.classList.add("col-resizing");

      const cleanup = () => {
        document.body.classList.remove("col-resizing");
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        activeDragCleanup.current = null;
      };
      const onMove = (moveEvent: MouseEvent) => {
        const delta = moveEvent.clientX - startX;
        const next =
          side === "left"
            ? { ...paneIntentsRef.current, left: Math.max(SIDEBAR_MIN_WIDTH, start.left + delta) }
            : side === "preview"
              ? {
                  ...paneIntentsRef.current,
                  preview: Math.max(PREVIEW_PANE_MIN_WIDTH, start.preview - delta),
                }
              : {
                  ...paneIntentsRef.current,
                  right: Math.max(RIGHT_PANE_MIN_WIDTH, start.right - delta),
                };
        setPaneIntents(next);
      };
      const onUp = () => {
        cleanup();
        void useAppStore.getState().patchState({
          sidebarWidth: paneIntentsRef.current.left,
          rightPaneWidth: paneIntentsRef.current.right,
          previewPaneWidth: paneIntentsRef.current.preview,
        });
      };

      activeDragCleanup.current = cleanup;
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    },
    [],
  );

  const restorePaneIntents = useCallback((state: Record<string, unknown>) => {
    const left = state.sidebarWidth;
    const right = state.rightPaneWidth;
    const preview = state.previewPaneWidth;
    setPaneIntents((current) => ({
      left: typeof left === "number" && Number.isFinite(left) ? left : current.left,
      right: typeof right === "number" && Number.isFinite(right) ? right : current.right,
      preview:
        typeof preview === "number" && Number.isFinite(preview) ? preview : current.preview,
    }));
  }, []);

  return { contentRowRef, paneWidths, beginPaneDrag, restorePaneIntents };
}

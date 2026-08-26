// Private geometry state for the main shell's adjustable panes. Persisted
// values are intent; rendered widths are clamped against the live container.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { useAppStore } from "../state/app-store";
import {
  GRID_MIN_WIDTH,
  PREVIEW_PANE_DEFAULT_RATIO,
  PREVIEW_PANE_MIN_WIDTH,
  RIGHT_PANE_DEFAULT_WIDTH,
  RIGHT_PANE_MIN_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SPLITTER_WIDTH,
  computeMinWindowWidth,
  derivePaneWidths,
} from "../utils/windowSizing";

interface PaneIntents {
  left: number;
  right: number;
  previewRatio: number;
}

export function usePaneLayout(splitOpen: boolean) {
  const [paneIntents, setPaneIntents] = useState<PaneIntents>({
    left: SIDEBAR_DEFAULT_WIDTH,
    right: RIGHT_PANE_DEFAULT_WIDTH,
    previewRatio: PREVIEW_PANE_DEFAULT_RATIO,
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

  const paneWidths = derivePaneWidths(
    paneIntents.left,
    paneIntents.right,
    containerWidth ?? computeMinWindowWidth() * 4,
    splitOpen ? paneIntents.previewRatio : null,
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
      const rowWidth = contentRowRef.current?.clientWidth ?? containerWidth;
      if (rowWidth === null) return;
      const startIntents = { ...paneIntentsRef.current };
      const startWidths = derivePaneWidths(
        startIntents.left,
        startIntents.right,
        rowWidth,
        splitOpen ? startIntents.previewRatio : null,
      );
      const peerWidth = Math.max(
        GRID_MIN_WIDTH + PREVIEW_PANE_MIN_WIDTH,
        rowWidth - startWidths.left - startWidths.right - 3 * SPLITTER_WIDTH,
      );
      document.body.classList.add("col-resizing");

      const cleanup = () => {
        document.body.classList.remove("col-resizing");
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        activeDragCleanup.current = null;
      };
      const intentsAt = (clientX: number): PaneIntents => {
        const delta = clientX - startX;
        return side === "left"
          ? {
              ...startIntents,
              left: Math.max(SIDEBAR_MIN_WIDTH, startWidths.left + delta),
            }
          : side === "preview"
            ? {
                ...startIntents,
                previewRatio:
                  Math.min(
                    peerWidth - GRID_MIN_WIDTH,
                    Math.max(PREVIEW_PANE_MIN_WIDTH, startWidths.preview - delta),
                  ) / peerWidth,
              }
            : {
                ...startIntents,
                right: Math.max(RIGHT_PANE_MIN_WIDTH, startWidths.right - delta),
              };
      };
      const onMove = (moveEvent: MouseEvent) => {
        setPaneIntents(intentsAt(moveEvent.clientX));
      };
      const onUp = (upEvent: MouseEvent) => {
        const finalIntents = intentsAt(upEvent.clientX);
        setPaneIntents(finalIntents);
        cleanup();
        const patch =
          side === "left"
            ? { sidebarWidth: finalIntents.left }
            : side === "right"
              ? { rightPaneWidth: finalIntents.right }
              : { previewPaneRatio: finalIntents.previewRatio };
        void useAppStore.getState().patchState(patch);
      };

      activeDragCleanup.current = cleanup;
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    },
    [containerWidth, splitOpen],
  );

  const restorePaneIntents = useCallback((state: Record<string, unknown>) => {
    const left = state.sidebarWidth;
    const right = state.rightPaneWidth;
    const previewRatio = state.previewPaneRatio;
    setPaneIntents((current) => ({
      left: typeof left === "number" && Number.isFinite(left) ? left : current.left,
      right: typeof right === "number" && Number.isFinite(right) ? right : current.right,
      previewRatio:
        typeof previewRatio === "number" &&
        Number.isFinite(previewRatio) &&
        previewRatio > 0 &&
        previewRatio < 1
          ? previewRatio
          : current.previewRatio,
    }));
  }, []);

  return { contentRowRef, paneWidths, beginPaneDrag, restorePaneIntents };
}

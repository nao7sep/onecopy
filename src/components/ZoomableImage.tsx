import { useEffect, useRef, useState } from "react";
import { originalUrl, previewUrl } from "../models/items";

// A fit-view image (cached preview) with a true 100% mode over the original
// bytes (mediafile range protocol) and drag-panning. Z or click toggles;
// double-click returns to fit. Shared by the preview window and the
// comparison per-slot enlarge.

export default function ZoomableImage({
  hash,
  fileName,
  onError,
}: {
  hash: string;
  fileName: string;
  /** Fires when the fit-view preview fails to load (missing/undecodable) so
   * the host can show words instead of the webview's broken-image icon. */
  onError?: () => void;
}) {
  const [zoomed, setZoomed] = useState(false);
  const panRef = useRef<HTMLDivElement | null>(null);
  const dragState = useRef<{ x: number; y: number; left: number; top: number } | null>(null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() === "z") {
        event.preventDefault();
        setZoomed((z) => !z);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  if (!zoomed) {
    return (
      <img
        src={previewUrl(hash)}
        alt={fileName}
        title="Click or press Z for 100%"
        className="max-h-full max-w-full cursor-zoom-in object-contain"
        onClick={() => setZoomed(true)}
        onError={onError}
      />
    );
  }
  return (
    <div
      ref={panRef}
      className="h-full w-full cursor-grab overflow-auto active:cursor-grabbing"
      onMouseDown={(e) => {
        const pane = panRef.current;
        if (!pane) return;
        dragState.current = {
          x: e.clientX,
          y: e.clientY,
          left: pane.scrollLeft,
          top: pane.scrollTop,
        };
      }}
      onMouseMove={(e) => {
        const pane = panRef.current;
        const drag = dragState.current;
        if (!pane || !drag || e.buttons === 0) return;
        pane.scrollLeft = drag.left - (e.clientX - drag.x);
        pane.scrollTop = drag.top - (e.clientY - drag.y);
      }}
      onMouseUp={() => (dragState.current = null)}
      onDoubleClick={() => setZoomed(false)}
    >
      <img
        src={originalUrl(hash)}
        alt={fileName}
        title="Drag to pan · double-click or Z to fit"
        className="max-w-none"
        draggable={false}
      />
    </div>
  );
}

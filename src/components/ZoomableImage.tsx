import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  fullresUrl,
  needsConvertedFullres,
  originalUrl,
  previewUrl,
} from "../models/items";
import { log, toErrorFields } from "../repositories";

// A fit-view image (cached preview) with a true 100% mode over the original
// bytes (mediafile range protocol) and drag-panning. Z or click toggles;
// double-click returns to fit. Shared by the preview window and the
// comparison per-slot enlarge.

export default function ZoomableImage({
  hash,
  fileName,
  startZoomed = false,
  onError,
}: {
  hash: string;
  fileName: string;
  /** Enter's "inspect": mount already at 100% (Space's peek never sets it). */
  startZoomed?: boolean;
  /** Fires when the fit-view preview fails to load (missing/undecodable) so
   * the host can show words instead of the webview's broken-image icon. */
  onError?: () => void;
}) {
  const [zoomed, setZoomed] = useState(startZoomed);
  /** For HEIC/AVIF the 100% source is the CONVERTED cache entry, ready only
   * after `ensure_fullres` lands; null while converting. Other formats read
   * the original directly and never touch this. */
  const [convertedSrc, setConvertedSrc] = useState<string | null>(null);
  const converted = needsConvertedFullres(fileName);
  useEffect(() => {
    // Enter while the surface is already open re-inspects the same photo;
    // the flag only ever pushes INTO 100% — Z and double-click stay the way
    // back, and it never forces fit on a photo the user zoomed themselves.
    if (startZoomed) setZoomed(true);
  }, [startZoomed]);
  useEffect(() => {
    if (!zoomed || !converted || convertedSrc !== null) return;
    let stale = false;
    void invoke("ensure_fullres", { hash })
      .then(() => {
        if (!stale) setConvertedSrc(fullresUrl(hash));
      })
      .catch((error) => {
        log.warn("fullres conversion failed", toErrorFields(error));
        // Fall back to the original bytes — WKWebView paints HEIC natively,
        // so on macOS this still shows something rather than nothing.
        if (!stale) setConvertedSrc(originalUrl(hash));
      });
    return () => {
      stale = true;
    };
  }, [zoomed, converted, convertedSrc, hash]);
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
      {converted && convertedSrc === null ? (
        <p className="p-4 text-sm text-ink-muted">Converting for 100% view…</p>
      ) : (
        <img
          src={converted ? (convertedSrc ?? "") : originalUrl(hash)}
          alt={fileName}
          title="Drag to pan · double-click or Z to fit"
          className="max-w-none"
          draggable={false}
        />
      )}
    </div>
  );
}

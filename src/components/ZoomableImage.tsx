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
// bytes (mediafile range protocol) and position-mapped panning. Z or click
// toggles; double-click returns to fit. Shared by every fit-view image
// surface.

/** Cursor position → pan position, with an edge margin: the outer 6% of the
 * pane already reads as fully-there, so a corner never needs pixel-perfect
 * aim (the exact edge pixel is hard to hit and the OS cursor may not even
 * deliver it). Clamped to [0, 1]. */
export function panFraction(position: number, extent: number): number {
  if (extent <= 0) return 0;
  const margin = extent * 0.06;
  const usable = extent - margin * 2;
  if (usable <= 0) return 0.5;
  return Math.min(1, Math.max(0, (position - margin) / usable));
}

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
  /** For HEIC/AVIF the 100% source is the CONVERTED cache entry, ready only
   * after `ensure_fullres` lands; null while converting. Other formats read
   * the original directly and never touch this. */
  const [convertedSrc, setConvertedSrc] = useState<string | null>(null);
  const converted = needsConvertedFullres(fileName);
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
      className="h-full w-full cursor-crosshair overflow-auto"
      // Position-mapped panning (developer, 2026-08-17, replacing drag): the
      // cursor's place IN THE PANE is the place IN THE IMAGE — top-left shows
      // top-left, bottom-right shows bottom-right, continuously. Dragging a
      // mostly-hidden original corner-to-corner took several grab strokes;
      // this is one sweep. The mapping is proportional PER AXIS, which is
      // what keeps a panorama's far edge reachable — a same-rate-both-axes
      // mapping would preserve nothing (the zoom is uniform, so no aspect
      // distortion exists to prevent) and strand the long axis. The cursor
      // position is the view state, so wheel scrolling is deliberately left
      // to be overridden by the next mouse move.
      onMouseMove={(e) => {
        const pane = panRef.current;
        if (!pane) return;
        const rect = pane.getBoundingClientRect();
        pane.scrollLeft =
          panFraction(e.clientX - rect.left, rect.width) *
          (pane.scrollWidth - pane.clientWidth);
        pane.scrollTop =
          panFraction(e.clientY - rect.top, rect.height) *
          (pane.scrollHeight - pane.clientHeight);
      }}
      onDoubleClick={() => setZoomed(false)}
    >
      {converted && convertedSrc === null ? (
        <p className="p-4 text-sm text-ink-muted">Converting for 100% view…</p>
      ) : (
        <img
          src={converted ? (convertedSrc ?? "") : originalUrl(hash)}
          alt={fileName}
          title="Move the mouse to pan · double-click or Z to fit"
          className="max-w-none"
          draggable={false}
        />
      )}
    </div>
  );
}

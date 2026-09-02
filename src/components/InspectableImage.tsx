import { useEffect, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  fullresUrl,
  needsConvertedFullres,
  originalUrl,
  previewUrl,
} from "../models/items";
import { useHoldInspect, type PointerPoint } from "../hooks/useHoldInspect";
import { log, toErrorFields } from "../repositories";
import { recordActionFailure } from "../state/notifications-store";
import OperationResult from "./ui/OperationResult";

/** Cursor position mapped into the source, with a forgiving edge margin. */
export function panFraction(position: number, extent: number): number {
  if (extent <= 0) return 0;
  const margin = extent * 0.06;
  const usable = extent - margin * 2;
  if (usable <= 0) return 0.5;
  return Math.min(1, Math.max(0, (position - margin) / usable));
}

/** Source offset that centres small media and makes every large edge reachable. */
export function intrinsicOffset(
  sourceExtent: number,
  viewportExtent: number,
  fraction: number,
): number {
  if (sourceExtent <= viewportExtent)
    return (viewportExtent - sourceExtent) / 2;
  return -(sourceExtent - viewportExtent) * fraction;
}

export interface InspectPosition {
  x: number;
  y: number;
  width: number;
  height: number;
}

export function inspectPosition(
  viewport: HTMLElement | null,
  point: PointerPoint,
): InspectPosition {
  if (viewport === null) return { x: 0.5, y: 0.5, width: 0, height: 0 };
  const rect = viewport.getBoundingClientRect();
  return {
    x: panFraction(point.clientX - rect.left, rect.width),
    y: panFraction(point.clientY - rect.top, rect.height),
    width: rect.width,
    height: rect.height,
  };
}

export default function InspectableImage({
  hash,
  fileName,
  enlargeSmall,
  sourceWidth = null,
  sourceHeight = null,
  onError,
}: {
  hash: string;
  fileName: string;
  enlargeSmall: boolean;
  sourceWidth?: number | null;
  sourceHeight?: number | null;
  onError?: () => void;
}) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const [position, setPosition] = useState<InspectPosition>({
    x: 0.5,
    y: 0.5,
    width: 0,
    height: 0,
  });
  const [sourceSize, setSourceSize] = useState({ width: 0, height: 0 });
  const [originalFailed, setOriginalFailed] = useState(false);
  const [inspectionError, setInspectionError] = useState<string | null>(null);
  const converted = needsConvertedFullres(fileName);
  const [convertedSrc, setConvertedSrc] = useState<string | null>(null);
  useEffect(() => {
    setConvertedSrc(null);
    setOriginalFailed(false);
    setInspectionError(null);
    setSourceSize({ width: 0, height: 0 });
  }, [hash]);
  const updatePosition = (point: PointerPoint) => {
    setPosition(inspectPosition(viewportRef.current, point));
  };
  const hold = useHoldInspect({
    onStart: (point) => {
      setOriginalFailed(false);
      setInspectionError(null);
      updatePosition(point);
    },
    onMove: updatePosition,
    onEnd: () => undefined,
  });

  useEffect(() => {
    if (!hold.inspecting || !converted || convertedSrc !== null) return;
    let stale = false;
    void invoke("ensure_fullres", { hash })
      .then(() => {
        if (!stale) setConvertedSrc(fullresUrl(hash));
      })
      .catch((error) => {
        log.warn("fullres conversion failed", toErrorFields(error));
        if (!stale) setConvertedSrc(originalUrl(hash));
      });
    return () => {
      stale = true;
    };
  }, [converted, convertedSrc, hash, hold.inspecting]);

  const source = converted ? convertedSrc : originalUrl(hash);
  return (
    <div
      ref={viewportRef}
      className="relative flex h-full w-full items-center justify-center overflow-hidden"
      title="Press and hold for original pixels"
      onPointerDown={hold.onPointerDown}
      onClickCapture={hold.onClickCapture}
    >
      <img
        src={previewUrl(hash)}
        alt={fileName}
        className={`${
          enlargeSmall || (sourceWidth !== null && sourceHeight !== null)
            ? "h-full w-full"
            : "max-h-full max-w-full"
        } cursor-zoom-in object-contain`}
        style={
          !enlargeSmall && sourceWidth !== null && sourceHeight !== null
            ? { maxWidth: `${sourceWidth}px`, maxHeight: `${sourceHeight}px` }
            : undefined
        }
        draggable={false}
        onError={onError}
      />
      {hold.inspecting ? (
        <div className="absolute inset-0 cursor-crosshair overflow-hidden bg-background">
          {originalFailed ? (
            <p className="p-4 text-sm text-ink-muted">
              Original pixels unavailable.
            </p>
          ) : source === null ? (
            <p className="p-4 text-sm text-ink-muted">
              Preparing original pixels…
            </p>
          ) : (
            <img
              src={source}
              alt={`${fileName} at original size`}
              draggable={false}
              className="pointer-events-none absolute max-h-none max-w-none"
              onLoad={(event) => {
                setInspectionError(null);
                setSourceSize({
                  width: event.currentTarget.naturalWidth,
                  height: event.currentTarget.naturalHeight,
                });
              }}
              onError={() => {
                if (!originalFailed) {
                  const message = `Couldn’t show the original pixels for ${fileName}.`;
                  log.warn("original-pixel inspection failed", { hash, fileName });
                  recordActionFailure(
                    "original-pixels-failed",
                    message,
                  );
                  setInspectionError(message);
                }
                setOriginalFailed(true);
              }}
              style={originalStyle(position, sourceSize)}
            />
          )}
        </div>
      ) : null}
      {inspectionError !== null ? (
        <OperationResult
          level="error"
          className="absolute bottom-2 left-2 right-2 z-10 shadow-sm"
        >
          {inspectionError}
        </OperationResult>
      ) : null}
    </div>
  );
}

function originalStyle(
  position: InspectPosition,
  sourceSize: { width: number; height: number },
): CSSProperties {
  if (
    sourceSize.width === 0 ||
    sourceSize.height === 0 ||
    position.width <= 0 ||
    position.height <= 0
  ) {
    return { visibility: "hidden" };
  }
  return {
    width: `${sourceSize.width}px`,
    height: `${sourceSize.height}px`,
    left: `${intrinsicOffset(sourceSize.width, position.width, position.x)}px`,
    top: `${intrinsicOffset(sourceSize.height, position.height, position.y)}px`,
  };
}

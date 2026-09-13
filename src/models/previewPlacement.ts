import { monitorKey, type MonitorRect } from "../utils/screens";

export interface PreviewMonitor extends MonitorRect {}

export interface PreviewBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PreviewWindowPlacement {
  screen: string;
  normalBounds: PreviewBounds;
  mode: "normal" | "maximized";
}

/** Treat state.json as untrusted input at the feature boundary. */
export function previewWindowPlacementFromState(
  value: unknown,
): PreviewWindowPlacement | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  const bounds = record.normalBounds;
  if (typeof bounds !== "object" || bounds === null) return null;
  const rectangle = bounds as Record<string, unknown>;
  if (
    typeof record.screen !== "string"
    || record.screen.length === 0
    || (record.mode !== "normal" && record.mode !== "maximized")
    || ![rectangle.x, rectangle.y, rectangle.width, rectangle.height]
      .every((part) => typeof part === "number" && Number.isFinite(part))
    || (rectangle.width as number) <= 0
    || (rectangle.height as number) <= 0
  ) return null;
  return {
    screen: record.screen,
    mode: record.mode,
    normalBounds: {
      x: rectangle.x as number,
      y: rectangle.y as number,
      width: rectangle.width as number,
      height: rectangle.height as number,
    },
  };
}

/** Physical outer rectangles; configured priority is also the overlap tie-breaker. */
export function hostingScreen<T extends PreviewMonitor>(
  monitors: readonly T[],
  bounds: PreviewBounds,
): T | null {
  let best: T | null = null;
  let bestArea = 0;
  for (const monitor of monitors) {
    const width = Math.max(
      0,
      Math.min(bounds.x + bounds.width, monitor.position.x + monitor.size.width)
        - Math.max(bounds.x, monitor.position.x),
    );
    const height = Math.max(
      0,
      Math.min(bounds.y + bounds.height, monitor.position.y + monitor.size.height)
        - Math.max(bounds.y, monitor.position.y),
    );
    if (width * height > bestArea) {
      best = monitor;
      bestArea = width * height;
    }
  }
  return best;
}

function designedBounds(monitor: PreviewMonitor): PreviewBounds {
  const area = monitor.workArea ?? monitor;
  const scale = monitor.scaleFactor ?? 1;
  const width = Math.max(1, Math.floor(Math.min(1280 * scale, area.size.width * 0.8)));
  const height = Math.max(1, Math.floor(Math.min(800 * scale, area.size.height * 0.8)));
  return {
    x: Math.round(area.position.x + (area.size.width - width) / 2),
    y: Math.round(area.position.y + (area.size.height - height) / 2),
    width,
    height,
  };
}

function fittedBoundsOn(
  bounds: PreviewBounds,
  monitor: PreviewMonitor,
): PreviewBounds | null {
  const area = monitor.workArea ?? monitor;
  if (
    !Object.values(bounds).every(Number.isFinite)
    || bounds.width <= 0
    || bounds.height <= 0
    || bounds.width > monitor.size.width
    || bounds.height > monitor.size.height
  ) return null;
  const width = Math.min(bounds.width, area.size.width);
  const height = Math.min(bounds.height, area.size.height);
  return {
    x: Math.min(
      Math.max(bounds.x, area.position.x),
      area.position.x + area.size.width - width,
    ),
    y: Math.min(
      Math.max(bounds.y, area.position.y),
      area.position.y + area.size.height - height,
    ),
    width,
    height,
  };
}

/** Native macOS zoom is not reliably exposed as WindowMaximized by every backend. */
export function isMaximizedPreviewBounds(
  bounds: PreviewBounds,
  monitor: PreviewMonitor,
): boolean {
  const area = monitor.workArea ?? monitor;
  const tolerance = 8 * (monitor.scaleFactor ?? 1);
  return bounds.x <= area.position.x + tolerance
    && bounds.y <= area.position.y + tolerance
    && bounds.x + bounds.width >= area.position.x + area.size.width - tolerance
    && bounds.y + bounds.height >= area.position.y + area.size.height - tolerance;
}

/** A saved user placement wins while its display exists; otherwise allocate afresh. */
export function allocatePreviewPlacement(
  orderedMonitors: readonly PreviewMonitor[],
  mainBounds: PreviewBounds,
  remembered: PreviewWindowPlacement | null,
): PreviewWindowPlacement | null {
  const main = hostingScreen(orderedMonitors, mainBounds) ?? orderedMonitors[0];
  if (!main) return null;
  const retained = orderedMonitors.find(
    (monitor) => monitorKey(monitor) === remembered?.screen,
  );
  const target = retained ?? orderedMonitors.find((monitor) => monitor !== main) ?? main;
  const retainedBounds = retained === undefined || remembered === null
    ? null
    : fittedBoundsOn(remembered.normalBounds, retained);
  return {
    screen: monitorKey(target),
    normalBounds: retainedBounds
      ? retainedBounds
      : designedBounds(target),
    mode: retained ? remembered!.mode : target === main ? "normal" : "maximized",
  };
}

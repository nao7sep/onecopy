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

function usableOn(bounds: PreviewBounds, monitor: PreviewMonitor): boolean {
  const area = monitor.workArea ?? monitor;
  return bounds.width > 0
    && bounds.height > 0
    && bounds.x >= area.position.x
    && bounds.y >= area.position.y
    && bounds.x + bounds.width <= area.position.x + area.size.width
    && bounds.y + bounds.height <= area.position.y + area.size.height;
}

/** Session state is the only remembered input; durable legacy Preview bounds are never read. */
export function allocatePreviewPlacement(
  orderedMonitors: readonly PreviewMonitor[],
  mainBounds: PreviewBounds,
  session: PreviewWindowPlacement | null,
): PreviewWindowPlacement | null {
  const main = hostingScreen(orderedMonitors, mainBounds) ?? orderedMonitors[0];
  if (!main) return null;
  const retained = orderedMonitors.find(
    (monitor) => monitorKey(monitor) === session?.screen && monitor !== main,
  );
  const target = retained ?? orderedMonitors.find((monitor) => monitor !== main) ?? main;
  return {
    screen: monitorKey(target),
    normalBounds: retained && usableOn(session!.normalBounds, target)
      ? { ...session!.normalBounds }
      : designedBounds(target),
    mode: retained ? session!.mode : target === main ? "normal" : "maximized",
  };
}

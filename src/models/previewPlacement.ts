import { monitorKey, type MonitorLike } from "../utils/screens";
import { restorableBounds, type MonitorRect, type SavedBounds, type WindowPlacementRecord } from "../utils/windowBounds";

export interface PreviewMonitor extends MonitorLike, MonitorRect {}
export interface PreviewWindowPlacement extends WindowPlacementRecord {
  screen: string;
}

/** Physical outer rectangles; priority order is also the overlap tie-breaker. */
export function hostingScreen<T extends PreviewMonitor>(monitors: readonly T[], bounds: SavedBounds): T | null {
  let best: T | null = null;
  let bestArea = 0;
  for (const monitor of monitors) {
    const width = Math.max(0, Math.min(bounds.x + bounds.width, monitor.position.x + monitor.size.width) - Math.max(bounds.x, monitor.position.x));
    const height = Math.max(0, Math.min(bounds.y + bounds.height, monitor.position.y + monitor.size.height) - Math.max(bounds.y, monitor.position.y));
    if (width * height > bestArea) {
      best = monitor;
      bestArea = width * height;
    }
  }
  return best;
}

function designedBounds(monitor: PreviewMonitor): SavedBounds {
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

/** Session state is the only remembered input; durable legacy bounds are not read. */
export function allocatePreviewPlacement(
  orderedMonitors: readonly PreviewMonitor[],
  mainBounds: SavedBounds,
  session: PreviewWindowPlacement | null,
): PreviewWindowPlacement | null {
  const main = hostingScreen(orderedMonitors, mainBounds) ?? orderedMonitors[0];
  if (!main) return null;
  const retained = orderedMonitors.find((monitor) => monitorKey(monitor) === session?.screen && monitor !== main);
  const target = retained ?? orderedMonitors.find((monitor) => monitor !== main) ?? main;
  return {
    screen: monitorKey(target),
    normalBounds: (retained ? restorableBounds(session?.normalBounds ?? null, [target]) : null) ?? designedBounds(target),
    mode: retained ? session!.mode : target === main ? "normal" : "maximized",
  };
}

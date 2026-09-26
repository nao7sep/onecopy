import type { MonitorRect } from "../utils/screens";

export interface PreviewMonitor extends MonitorRect {}

export interface PreviewBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PreviewInitialPlacement {
  normalBounds: PreviewBounds;
  maximized: boolean;
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

/** First-use policy only; Rust owns every later restore and capture. */
export function allocatePreviewPlacement(
  orderedMonitors: readonly PreviewMonitor[],
  mainBounds: PreviewBounds,
): PreviewInitialPlacement | null {
  // When Main's own display cannot be determined, nothing is excluded (screen-
  // priority.md: "the order is a preference over the displays that happen to
  // be connected"; the fallback below is not a claim that the first entry is
  // Main's display, so it must not be dropped from consideration, D-S11).
  const main = hostingScreen(orderedMonitors, mainBounds);
  const target = main === null
    ? orderedMonitors[0]
    : orderedMonitors.find((monitor) => monitor !== main) ?? main;
  if (!target) return null;
  return {
    normalBounds: designedBounds(target),
    maximized: main !== null && target !== main,
  };
}

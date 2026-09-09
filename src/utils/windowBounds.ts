// Durable-window placement decisions and the Tauri event edge.
//
// Bounds are physical outer-window coordinates throughout: Tauri reports
// monitor work areas, outer positions, and outer sizes in that coordinate
// space. Normal geometry and the stable normal/maximized mode are independent;
// minimized and fullscreen remain transient.

import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";

export interface SavedBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type WindowPlacementMode = "normal" | "maximized";

export interface WindowPlacementRecord {
  normalBounds: SavedBounds | null;
  mode: WindowPlacementMode;
}

export interface MonitorRect {
  position: { x: number; y: number };
  size: { width: number; height: number };
  scaleFactor?: number;
  workArea?: {
    position: { x: number; y: number };
    size: { width: number; height: number };
  };
}

export interface PlacementWindow {
  outerPosition: () => Promise<{ x: number; y: number }>;
  outerSize: () => Promise<{ width: number; height: number }>;
  setPosition: (position: PhysicalPosition) => Promise<void>;
  setSize: (size: PhysicalSize) => Promise<void>;
  isMinimized: () => Promise<boolean>;
  isFullscreen: () => Promise<boolean>;
  isMaximized: () => Promise<boolean>;
  maximize: () => Promise<void>;
  onMoved: (handler: () => void) => Promise<() => void>;
  onResized: (handler: () => void) => Promise<() => void>;
}

export interface WindowPlacementController {
  activate: () => Promise<void>;
  flush: () => Promise<void>;
  dispose: () => void;
}

const CAPTURE_DEBOUNCE_MS = 400;
const RESTORATION_SETTLE_MS = 500;

export function parseSavedBounds(value: unknown): SavedBounds | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const values = [record.x, record.y, record.width, record.height];
  if (!values.every((value) => typeof value === "number" && Number.isFinite(value))) {
    return null;
  }
  const [x, y, width, height] = values as number[];
  if (!values.every(Number.isInteger) || width < 1 || height < 1) return null;
  return { x, y, width, height };
}

export function placementFromLegacyState(
  bounds: unknown,
  maximized: unknown,
  defaultMode: WindowPlacementMode,
): WindowPlacementRecord {
  return {
    normalBounds: parseSavedBounds(bounds),
    mode: maximized === true ? "maximized" : maximized === false ? "normal" : defaultMode,
  };
}

export function restorableBounds(
  saved: SavedBounds | null,
  monitors: readonly MonitorRect[],
  minimum: { width: number; height: number } = { width: 1, height: 1 },
): SavedBounds | null {
  if (saved === null) return null;
  const values = [saved.x, saved.y, saved.width, saved.height];
  if (!values.every(Number.isFinite) || !values.every(Number.isInteger)) return null;
  const fits = monitors.some((monitor) => {
    const area = monitor.workArea ?? { position: monitor.position, size: monitor.size };
    const scale = monitor.scaleFactor ?? 1;
    return saved.width >= Math.ceil(minimum.width * scale)
      && saved.height >= Math.ceil(minimum.height * scale)
      && saved.x >= area.position.x
      && saved.y >= area.position.y
      && saved.x + saved.width <= area.position.x + area.size.width
      && saved.y + saved.height <= area.position.y + area.size.height;
  });
  return fits ? { ...saved } : null;
}

export function settledWindowPlacement(
  previous: WindowPlacementRecord,
  snapshot: {
    bounds: SavedBounds;
    minimized: boolean;
    fullscreen: boolean;
    maximized: boolean;
  },
): WindowPlacementRecord {
  if (snapshot.minimized || snapshot.fullscreen) {
    return {
      normalBounds: previous.normalBounds ? { ...previous.normalBounds } : null,
      mode: previous.mode,
    };
  }
  if (snapshot.maximized) {
    return {
      normalBounds: previous.normalBounds ? { ...previous.normalBounds } : null,
      mode: "maximized",
    };
  }
  return { normalBounds: { ...snapshot.bounds }, mode: "normal" };
}

async function currentSnapshot(window: PlacementWindow) {
  const [position, size, minimized, fullscreen, maximized] = await Promise.all([
    window.outerPosition(),
    window.outerSize(),
    window.isMinimized(),
    window.isFullscreen(),
    window.isMaximized(),
  ]);
  return {
    bounds: { x: position.x, y: position.y, width: size.width, height: size.height },
    minimized,
    fullscreen,
    maximized,
  };
}

export async function prepareWindowPlacement(options: {
  window: PlacementWindow;
  saved: WindowPlacementRecord;
  minimum: { width: number; height: number };
  monitors: readonly MonitorRect[];
  persist: (record: WindowPlacementRecord) => Promise<void>;
  beforeNormalCapture?: () => Promise<void>;
  /** App-owned non-Spaces fullscreen is not reported by Tauri isFullscreen. */
  isTransient?: () => boolean;
  report: (operation: string, error: unknown) => void;
}): Promise<WindowPlacementController> {
  const { window, saved, minimum, monitors, persist, beforeNormalCapture, report } = options;
  const openingPosition = await window.outerPosition();
  const openingSize = await window.outerSize();
  const usable = restorableBounds(saved.normalBounds, monitors, minimum);
  let normalBounds: SavedBounds = usable ?? {
    x: openingPosition.x,
    y: openingPosition.y,
    width: openingSize.width,
    height: openingSize.height,
  };
  let mode = saved.mode;

  if (usable !== null) {
    try {
      await window.setSize(new PhysicalSize(usable.width, usable.height));
      await window.setPosition(new PhysicalPosition(usable.x, usable.y));
      const restoredPosition = await window.outerPosition();
      const restoredSize = await window.outerSize();
      if (
        restoredPosition.x !== usable.x
        || restoredPosition.y !== usable.y
        || restoredSize.width !== usable.width
        || restoredSize.height !== usable.height
      ) {
        throw new Error("Tauri adjusted the restored window bounds");
      }
    } catch (error) {
      report("restore window bounds", error);
      normalBounds = {
        x: openingPosition.x,
        y: openingPosition.y,
        width: openingSize.width,
        height: openingSize.height,
      };
      try {
        await window.setSize(new PhysicalSize(openingSize.width, openingSize.height));
        await window.setPosition(new PhysicalPosition(openingPosition.x, openingPosition.y));
      } catch (fallbackError) {
        report("restore default window bounds", fallbackError);
      }
    }
  }

  if (mode === "maximized") {
    try {
      await window.maximize();
    } catch (error) {
      mode = "normal";
      report("restore maximized window", error);
    }
  }

  let enabled = false;
  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let eventTail = Promise.resolve();
  const unlistens: Array<() => void> = [];

  const cancel = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };
  const currentRecord = (): WindowPlacementRecord => ({
    normalBounds: { ...normalBounds },
    mode,
  });
  const capture = async () => {
    if (!enabled || disposed || options.isTransient?.()) return;
    const snapshot = await currentSnapshot(window);
    if (options.isTransient?.()) return;
    if (snapshot.minimized || snapshot.fullscreen || snapshot.maximized) {
      cancel();
      const next = settledWindowPlacement(currentRecord(), snapshot);
      if (next.mode !== mode) {
        mode = next.mode;
        await persist(currentRecord());
      }
      return;
    }
    cancel();
    timer = setTimeout(() => {
      timer = null;
      eventTail = eventTail
        .then(async () => {
          if (!enabled || disposed || options.isTransient?.()) return;
          let settled = await currentSnapshot(window);
          if (options.isTransient?.()) return;
          if (!settled.minimized && !settled.fullscreen && !settled.maximized) {
            await beforeNormalCapture?.();
            settled = await currentSnapshot(window);
            if (options.isTransient?.()) return;
          }
          const next = settledWindowPlacement(currentRecord(), settled);
          normalBounds = next.normalBounds ?? normalBounds;
          mode = next.mode;
          await persist(currentRecord());
        })
        .catch((error) => report("save window placement", error));
    }, CAPTURE_DEBOUNCE_MS);
  };
  const enqueueCapture = () => {
    eventTail = eventTail
      .then(capture)
      .catch((error) => report("inspect window placement", error));
  };

  for (const [operation, registration] of [
    ["listen for window moves", window.onMoved(enqueueCapture)],
    ["listen for window resizes", window.onResized(enqueueCapture)],
  ] as const) {
    try {
      unlistens.push(await registration);
    } catch (error) {
      report(operation, error);
    }
  }

  return {
    activate: async () => {
      if (enabled || disposed) return;
      await new Promise((resolve) => setTimeout(resolve, RESTORATION_SETTLE_MS));
      if (disposed) return;
      enabled = true;
      if (mode === "maximized" && !await window.isMaximized()) mode = "normal";
    },
    flush: async () => {
      cancel();
      await eventTail;
      if (!enabled || disposed || options.isTransient?.()) return;
      let snapshot = await currentSnapshot(window);
      if (options.isTransient?.()) return;
      if (!snapshot.minimized && !snapshot.fullscreen && !snapshot.maximized) {
        await beforeNormalCapture?.();
        snapshot = await currentSnapshot(window);
        if (options.isTransient?.()) return;
      }
      const next = settledWindowPlacement(currentRecord(), snapshot);
      normalBounds = next.normalBounds ?? normalBounds;
      mode = next.mode;
      await persist(currentRecord());
    },
    dispose: () => {
      disposed = true;
      enabled = false;
      cancel();
      for (const unlisten of unlistens) unlisten();
    },
  };
}

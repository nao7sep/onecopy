import { afterEach, describe, expect, it, vi } from "vitest";
import {
  parseSavedBounds,
  placementFromLegacyState,
  prepareWindowPlacement,
  restorableBounds,
  settledWindowPlacement,
  type PlacementWindow,
} from "../../src/utils/windowBounds";

const MONITOR = {
  position: { x: 0, y: 0 },
  size: { width: 2560, height: 1440 },
  workArea: { position: { x: 0, y: 30 }, size: { width: 2560, height: 1410 } },
  scaleFactor: 1,
};
const SECOND = {
  position: { x: -1920, y: 0 },
  size: { width: 1920, height: 1080 },
  workArea: { position: { x: -1920, y: 0 }, size: { width: 1920, height: 1040 } },
  scaleFactor: 2,
};

afterEach(() => vi.useRealTimers());

describe("saved placement decisions", () => {
  it("accepts only complete integral bounds and preserves the declared default mode", () => {
    expect(parseSavedBounds({ x: 10, y: 20, width: 800, height: 600 })).toEqual({
      x: 10,
      y: 20,
      width: 800,
      height: 600,
    });
    expect(parseSavedBounds({ x: 10.5, y: 20, width: 800, height: 600 })).toBeNull();
    expect(parseSavedBounds({ x: 10, y: 20, width: 800 })).toBeNull();
    expect(parseSavedBounds({ x: Number.NaN, y: 0, width: 800, height: 600 })).toBeNull();
    expect(placementFromLegacyState(null, undefined, "maximized")).toEqual({
      normalBounds: null,
      mode: "maximized",
    });
  });

  it("restores only a complete rectangle inside one work area and above its scaled minimum", () => {
    const primary = { x: 100, y: 100, width: 1400, height: 900 };
    const secondary = { x: -1800, y: 20, width: 1400, height: 900 };
    expect(restorableBounds(primary, [MONITOR], { width: 900, height: 600 })).toEqual(primary);
    expect(restorableBounds(secondary, [MONITOR, SECOND], { width: 600, height: 400 }))
      .toEqual(secondary);
    expect(restorableBounds(
      { x: 2450, y: 100, width: 1400, height: 900 },
      [MONITOR],
      { width: 900, height: 600 },
    )).toBeNull();
    expect(restorableBounds(
      { x: 100, y: 100, width: 2600, height: 900 },
      [MONITOR],
      { width: 900, height: 600 },
    )).toBeNull();
    expect(restorableBounds(
      { x: -1800, y: 20, width: 1100, height: 900 },
      [SECOND],
      { width: 600, height: 400 },
    )).toBeNull();
  });

  it("keeps normal bounds through maximized, minimized, and fullscreen states", () => {
    const normal = {
      normalBounds: { x: 100, y: 100, width: 1400, height: 900 },
      mode: "normal" as const,
    };
    const maximized = settledWindowPlacement(normal, {
      bounds: { x: 0, y: 30, width: 2560, height: 1410 },
      minimized: false,
      fullscreen: false,
      maximized: true,
    });
    expect(maximized).toEqual({ normalBounds: normal.normalBounds, mode: "maximized" });
    expect(settledWindowPlacement(maximized, {
      bounds: { x: 0, y: 0, width: 200, height: 100 },
      minimized: true,
      fullscreen: false,
      maximized: false,
    })).toEqual(maximized);
    expect(settledWindowPlacement(maximized, {
      bounds: { x: 0, y: 0, width: 2560, height: 1440 },
      minimized: false,
      fullscreen: true,
      maximized: false,
    })).toEqual(maximized);
  });
});

describe("the placement lifecycle", () => {
  it("suppresses restoration events and flushes current normal bounds inside the debounce", async () => {
    vi.useFakeTimers();
    const current = { x: 10, y: 40, width: 1120, height: 640 };
    let moved: (() => void) | null = null;
    let resized: (() => void) | null = null;
    let minimized = false;
    let fullscreen = false;
    let maximized = false;
    const persisted: unknown[] = [];
    const window: PlacementWindow = {
      outerPosition: async () => ({ x: current.x, y: current.y }),
      outerSize: async () => ({ width: current.width, height: current.height }),
      setPosition: async (position) => {
        current.x = position.x;
        current.y = position.y;
        moved?.();
      },
      setSize: async (size) => {
        current.width = size.width;
        current.height = size.height;
        resized?.();
      },
      isMinimized: async () => minimized,
      isFullscreen: async () => fullscreen,
      isMaximized: async () => maximized,
      maximize: async () => { maximized = true; resized?.(); },
      onMoved: async (handler) => { moved = handler; return () => { moved = null; }; },
      onResized: async (handler) => { resized = handler; return () => { resized = null; }; },
    };
    const controller = await prepareWindowPlacement({
      window,
      saved: {
        normalBounds: { x: 100, y: 120, width: 1400, height: 900 },
        mode: "normal",
      },
      minimum: { width: 900, height: 600 },
      monitors: [MONITOR],
      persist: async (record) => { persisted.push(record); },
      report: (operation, error) => { throw new Error(operation, { cause: error }); },
    });
    const activation = controller.activate();
    await vi.advanceTimersByTimeAsync(500);
    await activation;
    expect(persisted).toEqual([]);

    Object.assign(current, { x: 180, y: 200, width: 1500, height: 950 });
    (moved as (() => void) | null)?.();
    await vi.advanceTimersByTimeAsync(0);
    await controller.flush();
    expect(persisted.at(-1)).toEqual({
      normalBounds: { x: 180, y: 200, width: 1500, height: 950 },
      mode: "normal",
    });

    maximized = true;
    (resized as (() => void) | null)?.();
    await vi.advanceTimersByTimeAsync(0);
    expect(persisted.at(-1)).toEqual({
      normalBounds: { x: 180, y: 200, width: 1500, height: 950 },
      mode: "maximized",
    });

    minimized = true;
    maximized = false;
    Object.assign(current, { x: 0, y: 0, width: 200, height: 100 });
    await controller.flush();
    expect(persisted.at(-1)).toEqual({
      normalBounds: { x: 180, y: 200, width: 1500, height: 950 },
      mode: "maximized",
    });
    fullscreen = true;
    minimized = false;
    await controller.flush();
    expect(persisted.at(-1)).toEqual({
      normalBounds: { x: 180, y: 200, width: 1500, height: 950 },
      mode: "maximized",
    });
    fullscreen = false;
    Object.assign(current, { x: 220, y: 240, width: 1460, height: 920 });
    (resized as (() => void) | null)?.();
    await vi.advanceTimersByTimeAsync(400);
    expect(persisted.at(-1)).toEqual({
      normalBounds: { x: 220, y: 240, width: 1460, height: 920 },
      mode: "normal",
    });
    controller.dispose();
  });

  it("serializes a newer placement behind an older in-flight write", async () => {
    vi.useFakeTimers();
    const current = { x: 10, y: 40, width: 1120, height: 640 };
    let moved: (() => void) | null = null;
    let releaseFirst!: () => void;
    const firstWrite = new Promise<void>((resolve) => { releaseFirst = resolve; });
    const persistedX: number[] = [];
    const window: PlacementWindow = {
      outerPosition: async () => ({ x: current.x, y: current.y }),
      outerSize: async () => ({ width: current.width, height: current.height }),
      setPosition: async () => {},
      setSize: async () => {},
      isMinimized: async () => false,
      isFullscreen: async () => false,
      isMaximized: async () => false,
      maximize: async () => {},
      onMoved: async (handler) => { moved = handler; return () => { moved = null; }; },
      onResized: async () => () => {},
    };
    const controller = await prepareWindowPlacement({
      window,
      saved: { normalBounds: null, mode: "normal" },
      minimum: { width: 900, height: 600 },
      monitors: [MONITOR],
      persist: async (record) => {
        persistedX.push(record.normalBounds?.x ?? -1);
        if (persistedX.length === 1) await firstWrite;
      },
      report: (operation, error) => { throw new Error(operation, { cause: error }); },
    });
    const activation = controller.activate();
    await vi.advanceTimersByTimeAsync(500);
    await activation;

    current.x = 100;
    (moved as (() => void) | null)?.();
    await vi.advanceTimersByTimeAsync(400);
    expect(persistedX).toEqual([100]);

    current.x = 200;
    (moved as (() => void) | null)?.();
    releaseFirst();
    await vi.advanceTimersByTimeAsync(400);
    await controller.flush();
    expect(persistedX).toEqual([100, 200, 200]);
    controller.dispose();
  });
});

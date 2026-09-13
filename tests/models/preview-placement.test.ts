import { describe, expect, it } from "vitest";
import {
  allocatePreviewPlacement,
  hostingScreen,
  isMaximizedPreviewBounds,
  previewWindowPlacementFromState,
  type PreviewMonitor,
  type PreviewWindowPlacement,
} from "../../src/models/previewPlacement";
import { monitorKey, orderMonitors } from "../../src/utils/screens";

const screens: PreviewMonitor[] = [0, 1, 2].map((index) => ({
  name: "Identical model",
  position: { x: index * 1920, y: 0 },
  size: { width: 1920, height: 1080 },
  workArea: {
    position: { x: index * 1920, y: 30 },
    size: { width: 1920, height: 1050 },
  },
  scaleFactor: 1,
}));
const main = { x: 100, y: 50, width: 1200, height: 800 };

describe("durable Preview allocation", () => {
  it("uses configured priority excluding Main's actual screen", () => {
    expect(allocatePreviewPlacement(screens, main, null)?.screen)
      .toBe(monitorKey(screens[1]));
    const reordered = orderMonitors(screens, [
      monitorKey(screens[2]),
      monitorKey(screens[0]),
    ]);
    expect(allocatePreviewPlacement(reordered, main, null)?.screen)
      .toBe(monitorKey(screens[2]));
    expect(allocatePreviewPlacement(reordered, { ...main, x: 4000 }, null)?.screen)
      .toBe(monitorKey(screens[0]));
  });

  it("resolves a straddling Main by greatest overlap and priority ties", () => {
    expect(hostingScreen(screens, { ...main, x: 1800 })).toBe(screens[1]);
    const equal = { ...main, x: 1320 };
    expect(hostingScreen(screens, equal)).toBe(screens[0]);
    expect(hostingScreen([screens[1], screens[0]], equal)).toBe(screens[1]);
  });

  it("preserves an explicit saved display, usable bounds, and mode", () => {
    const saved: PreviewWindowPlacement = {
      screen: monitorKey(screens[2]),
      mode: "normal",
      normalBounds: { ...main, x: 4000 },
    };
    expect(allocatePreviewPlacement(screens, main, saved)).toEqual(saved);
  });

  it("accepts only complete durable placement state", () => {
    const placement: PreviewWindowPlacement = {
      screen: monitorKey(screens[2]),
      mode: "maximized",
      normalBounds: { ...main, x: 4000 },
    };
    expect(previewWindowPlacementFromState(placement)).toEqual(placement);
    expect(previewWindowPlacementFromState({ ...placement, mode: "fullscreen" })).toBeNull();
    expect(previewWindowPlacementFromState({ ...placement, normalBounds: { ...main, width: 0 } }))
      .toBeNull();
    expect(previewWindowPlacementFromState({ ...placement, screen: "" })).toBeNull();
  });

  it("retains a Mac edge-tiled rectangle by fitting its outer frame to the work area", () => {
    const saved: PreviewWindowPlacement = {
      screen: monitorKey(screens[1]),
      mode: "normal",
      normalBounds: { x: 1919, y: 0, width: 961, height: 1080 },
    };
    expect(allocatePreviewPlacement(screens, main, saved)).toEqual({
      screen: monitorKey(screens[1]),
      mode: "normal",
      normalBounds: { x: 1920, y: 30, width: 961, height: 1050 },
    });
  });

  it("recognizes work-area-sized native geometry as maximized", () => {
    expect(isMaximizedPreviewBounds(
      { x: 1920, y: 30, width: 1920, height: 1050 },
      screens[1],
    )).toBe(true);
    expect(isMaximizedPreviewBounds(
      { x: 1920, y: 30, width: 960, height: 1050 },
      screens[1],
    )).toBe(false);
  });

  it("reallocates only when the retained display disappears", () => {
    const saved = allocatePreviewPlacement(screens, main, null);
    expect(allocatePreviewPlacement(screens, { ...main, x: 2000 }, saved))
      .toEqual(saved);
    expect(allocatePreviewPlacement([screens[0], screens[2]], main, saved))
      .toMatchObject({ screen: monitorKey(screens[2]), mode: "maximized" });
  });

  it("uses a normal fallback on one display and preserves a saved mode", () => {
    const prior: PreviewWindowPlacement = {
      screen: monitorKey(screens[0]),
      mode: "maximized",
      normalBounds: { x: -9000, y: -9000, width: 1280, height: 800 },
    };
    const result = allocatePreviewPlacement([screens[0]], main, prior)!;
    expect(result.mode).toBe("maximized");
    expect(result.normalBounds.width).toBeLessThan(screens[0].workArea!.size.width);
    expect(result.normalBounds.x).toBeGreaterThanOrEqual(0);
    expect(allocatePreviewPlacement([screens[0]], main, null)?.mode).toBe("normal");
    expect(allocatePreviewPlacement([], main, prior)).toBeNull();
  });

  it("uses physical work-area coordinates on a mixed-scale display", () => {
    const monitor: PreviewMonitor = {
      ...screens[0],
      position: { x: -3000, y: -400 },
      size: { width: 3000, height: 2000 },
      workArea: { position: { x: -3000, y: -360 }, size: { width: 3000, height: 1960 } },
      scaleFactor: 2,
    };
    const result = allocatePreviewPlacement([screens[0], monitor], main, null)!;
    expect(result.mode).toBe("maximized");
    expect(result.normalBounds.width).toBe(2400);
    expect(result.normalBounds.x).toBeGreaterThanOrEqual(-3000);
  });
});

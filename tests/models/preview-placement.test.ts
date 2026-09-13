import { describe, expect, it } from "vitest";
import {
  allocatePreviewPlacement,
  hostingScreen,
  type PreviewMonitor,
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

describe("first-use Preview allocation", () => {
  it("uses configured priority excluding Main's actual screen", () => {
    expect(allocatePreviewPlacement(screens, main)?.normalBounds.x)
      .toBeGreaterThanOrEqual(screens[1].position.x);
    const reordered = orderMonitors(screens, [
      monitorKey(screens[2]),
      monitorKey(screens[0]),
    ]);
    expect(allocatePreviewPlacement(reordered, main)?.normalBounds.x)
      .toBeGreaterThanOrEqual(screens[2].position.x);
    expect(allocatePreviewPlacement(reordered, { ...main, x: 4000 })?.normalBounds.x)
      .toBeLessThan(screens[1].position.x);
  });

  it("resolves a straddling Main by greatest overlap and priority ties", () => {
    expect(hostingScreen(screens, { ...main, x: 1800 })).toBe(screens[1]);
    const equal = { ...main, x: 1320 };
    expect(hostingScreen(screens, equal)).toBe(screens[0]);
    expect(hostingScreen([screens[1], screens[0]], equal)).toBe(screens[1]);
  });

  it("uses a normal fallback on one display", () => {
    const result = allocatePreviewPlacement([screens[0]], main)!;
    expect(result.maximized).toBe(false);
    expect(result.normalBounds.width).toBeLessThan(screens[0].workArea!.size.width);
    expect(result.normalBounds.x).toBeGreaterThanOrEqual(0);
    expect(allocatePreviewPlacement([], main)).toBeNull();
  });

  it("uses physical work-area coordinates on a mixed-scale display", () => {
    const monitor: PreviewMonitor = {
      ...screens[0],
      position: { x: -3000, y: -400 },
      size: { width: 3000, height: 2000 },
      workArea: { position: { x: -3000, y: -360 }, size: { width: 3000, height: 1960 } },
      scaleFactor: 2,
    };
    const result = allocatePreviewPlacement([screens[0], monitor], main)!;
    expect(result.maximized).toBe(true);
    expect(result.normalBounds.width).toBe(2400);
    expect(result.normalBounds.x).toBeGreaterThanOrEqual(-3000);
  });
});

import { describe, expect, it } from "vitest";
import { allocatePreviewPlacement, hostingScreen, type PreviewMonitor, type PreviewWindowPlacement } from "../../src/models/previewPlacement";
import { monitorKey, orderMonitors } from "../../src/utils/screens";
import { restorableBounds } from "../../src/utils/windowBounds";

const screens: PreviewMonitor[] = [0, 1, 2].map((index) => ({
  name: "Identical model", position: { x: index * 1920, y: 0 }, size: { width: 1920, height: 1080 },
  workArea: { position: { x: index * 1920, y: 30 }, size: { width: 1920, height: 1050 } }, scaleFactor: 1,
}));
const main = { x: 100, y: 50, width: 1200, height: 800 };

describe("session-aware Preview allocation", () => {
  it("uses configured priority excluding Main's actual screen, not a fixed second screen", () => {
    expect(allocatePreviewPlacement(screens, main, null)?.screen).toBe(monitorKey(screens[1]));
    const reordered = orderMonitors(screens, [monitorKey(screens[2]), monitorKey(screens[0])]);
    expect(allocatePreviewPlacement(reordered, main, null)?.screen).toBe(monitorKey(screens[2]));
    expect(allocatePreviewPlacement(reordered, { ...main, x: 4000 }, null)?.screen).toBe(monitorKey(screens[0]));
  });

  it("resolves straddling by overlap, with configured priority as tie-breaker", () => {
    expect(hostingScreen(screens, { ...main, x: 1800 })).toBe(screens[1]);
    const equal = { ...main, x: 1320 };
    expect(hostingScreen(screens, equal)).toBe(screens[0]);
    expect(hostingScreen([screens[1], screens[0]], equal)).toBe(screens[1]);
  });

  it("preserves an explicitly chosen third screen, normal bounds and mode within the session", () => {
    const session: PreviewWindowPlacement = {
      screen: monitorKey(screens[2]), mode: "normal", normalBounds: { ...main, x: 4000 },
    };
    expect(allocatePreviewPlacement(screens, main, session)).toEqual(session);
    expect(allocatePreviewPlacement(screens, main, null)?.screen).toBe(monitorKey(screens[1]));
  });

  it("reallocates when Main moves onto the remembered screen or that screen disappears", () => {
    const session = allocatePreviewPlacement(screens, main, null);
    expect(allocatePreviewPlacement(screens, { ...main, x: 2000 }, session))
      .toMatchObject({ screen: monitorKey(screens[0]), mode: "maximized" });
    expect(allocatePreviewPlacement([screens[0], screens[2]], main, session))
      .toMatchObject({ screen: monitorKey(screens[2]), mode: "maximized" });
  });

  it("rejects stale normal bounds as a unit but retains the session's separate-screen mode", () => {
    const session = { screen: monitorKey(screens[1]), mode: "normal" as const, normalBounds: main };
    const result = allocatePreviewPlacement(screens, main, session)!;
    expect(result.mode).toBe("normal");
    expect(restorableBounds(result.normalBounds, [screens[1]])).toEqual(result.normalBounds);
    expect(result.normalBounds).not.toEqual(main);
  });

  it("floats on a single screen with useful normal bounds, even after multi-screen maximization", () => {
    const prior = allocatePreviewPlacement(screens, main, null);
    const result = allocatePreviewPlacement([screens[0]], main, prior)!;
    expect(result.mode).toBe("normal");
    expect(result.normalBounds!.width).toBeLessThan(screens[0].workArea!.size.width);
    expect(restorableBounds(result.normalBounds, [screens[0]])).toEqual(result.normalBounds);
    expect(allocatePreviewPlacement([], main, prior)).toBeNull();
  });

  it("sizes fresh normal bounds in physical work-area coordinates on mixed-scale negative displays", () => {
    const monitor = { ...screens[0], position: { x: -3000, y: -400 }, size: { width: 3000, height: 2000 },
      workArea: { position: { x: -3000, y: -360 }, size: { width: 3000, height: 1960 } }, scaleFactor: 2 };
    const result = allocatePreviewPlacement([screens[0], monitor], main, null)!;
    expect(result.mode).toBe("maximized");
    expect(restorableBounds(result.normalBounds, [monitor])).toEqual(result.normalBounds);
    expect(result.normalBounds!.width).toBe(2400);
  });
});

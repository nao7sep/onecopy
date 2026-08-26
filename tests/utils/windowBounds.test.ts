// Window-bounds persistence, the pure half (windowBounds.ts).
//
// The stakes: these decide where the main window appears on every launch.
// Wrong acceptance restores a window onto a monitor that is no longer
// attached — no reachable title bar, an app that looks broken until the
// state file is hand-edited.

import { describe, expect, it } from "vitest";
import {
  parseSavedBounds,
  restorableBounds,
  shrinkToFit,
} from "../../src/utils/windowBounds";

const MONITOR = { position: { x: 0, y: 0 }, size: { width: 2560, height: 1440 } };
const SECOND = { position: { x: 2560, y: 0 }, size: { width: 1920, height: 1080 } };
const SCALED_LAPTOP = {
  position: { x: 0, y: 0 },
  size: { width: 1920, height: 1080 },
  workArea: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1032 } },
};

describe("parseSavedBounds", () => {
  it("accepts the saved shape and nothing weaker", () => {
    expect(parseSavedBounds({ x: 10, y: 20, width: 800, height: 600 })).toEqual({
      x: 10,
      y: 20,
      width: 800,
      height: 600,
    });
    // state.json survives hand edits and version skew: anything malformed is
    // a clean "nothing saved", never a throw.
    expect(parseSavedBounds(null)).toBeNull();
    expect(parseSavedBounds("1400x900")).toBeNull();
    expect(parseSavedBounds({ x: 10, y: 20, width: 800 })).toBeNull();
    expect(parseSavedBounds({ x: NaN, y: 0, width: 800, height: 600 })).toBeNull();
    expect(parseSavedBounds({ x: 0, y: 0, width: 0, height: 600 })).toBeNull();
  });

  it("allows negative positions — a monitor left of primary is negative x", () => {
    expect(parseSavedBounds({ x: -2560, y: 0, width: 800, height: 600 })).not.toBeNull();
  });
});

describe("restorableBounds", () => {
  it("keeps bounds whose top strip is grabbable on some monitor", () => {
    const saved = { x: 100, y: 100, width: 1400, height: 900 };
    expect(restorableBounds(saved, [MONITOR])).toEqual(saved);
    // On the second monitor of two.
    const onSecond = { x: 2700, y: 50, width: 1400, height: 900 };
    expect(restorableBounds(onSecond, [MONITOR, SECOND])).toEqual(onSecond);
  });

  it("rejects bounds on a monitor that is no longer attached", () => {
    // Saved while a second monitor existed; today only the first is here.
    expect(
      restorableBounds({ x: 2700, y: 50, width: 1400, height: 900 }, [MONITOR]),
    ).toBeNull();
  });

  it("rejects a window whose title bar sits above every screen", () => {
    // The body overlaps plenty, but the drag handle is off the top — the
    // exact stranding the top-strip rule exists for.
    expect(
      restorableBounds({ x: 100, y: -400, width: 1400, height: 900 }, [MONITOR]),
    ).toBeNull();
  });

  it("brings a grabbable, partly stranded window back inside the monitor", () => {
    const saved = { x: 2560 - 150, y: 1440 - 60, width: 1400, height: 900 };
    expect(restorableBounds(saved, [MONITOR])).toEqual({
      x: 1160,
      y: 540,
      width: 1400,
      height: 900,
    });
  });

  it("fits old oversized bounds to the current scaled work area", () => {
    expect(
      restorableBounds({ x: 20, y: 20, width: 2100, height: 1350 }, [SCALED_LAPTOP]),
    ).toEqual({
      x: 20,
      y: 20,
      width: 1728,
      height: 929,
    });
  });

  it("uses the taskbar-excluding work area for position clamps", () => {
    expect(
      restorableBounds({ x: 100, y: 300, width: 1400, height: 900 }, [SCALED_LAPTOP]),
    ).toEqual({ x: 100, y: 132, width: 1400, height: 900 });
  });

  it("passes null through", () => {
    expect(restorableBounds(null, [MONITOR])).toBeNull();
  });
});

describe("shrinkToFit", () => {
  it("leaves a window that fits alone", () => {
    expect(shrinkToFit({ width: 1400, height: 900 }, { width: 2560, height: 1440 })).toBeNull();
  });

  it("shrinks an overflowing default to 90% of the monitor", () => {
    // The 1280×800 laptop case: the config's 1400×900 default overflows.
    expect(shrinkToFit({ width: 1400, height: 900 }, { width: 1280, height: 800 })).toEqual({
      width: 1152,
      height: 720,
    });
  });

  it("shrinks only the overflowing axis", () => {
    expect(shrinkToFit({ width: 3000, height: 700 }, { width: 2560, height: 1440 })).toEqual({
      width: 2304,
      height: 700,
    });
  });
});

// The zoom steppers snap through nearest() before moving, so an off-ladder
// value does not step from where it looks like it is.

import { describe, expect, it } from "vitest";
import {
  ZOOM_LEVELS,
  ZOOM_MAX,
  ZOOM_MIN,
  stepZoomIn,
  stepZoomOut,
} from "../../src/utils/zoom";

describe("the zoom ladder", () => {
  it("steps from the NEAREST level, not from the raw value", () => {
    // 1.05 is not on the ladder. It snaps to 1.0 first, so stepping up lands
    // on 1.2 rather than somewhere just above 1.05.
    expect(stepZoomIn(1.05)).toBe(1.2);
    expect(stepZoomOut(1.05)).toBe(0.9);
  });

  it("moves one rung at a time from an on-ladder value", () => {
    expect(stepZoomIn(1.0)).toBe(1.2);
    expect(stepZoomOut(1.0)).toBe(0.9);
  });

  it("clamps at both ends instead of running off the ladder", () => {
    expect(stepZoomIn(ZOOM_MAX)).toBe(ZOOM_MAX);
    expect(stepZoomOut(ZOOM_MIN)).toBe(ZOOM_MIN);
    expect(stepZoomIn(999)).toBe(ZOOM_MAX);
    expect(stepZoomOut(0.01)).toBe(ZOOM_MIN);
  });

  it("keeps the ladder ascending and containing 100%", () => {
    const ascending = [...ZOOM_LEVELS].sort((a, b) => a - b);
    expect(ZOOM_LEVELS).toEqual(ascending);
    expect(ZOOM_LEVELS).toContain(1.0);
  });
});

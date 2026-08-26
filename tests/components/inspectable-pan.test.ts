// The held original-pixel view maps the cursor's pane position into the image,
// so crossing a mostly hidden original takes one sweep of the mouse.

import { describe, expect, it } from "vitest";
import { intrinsicOffset, panFraction } from "../../src/components/InspectableImage";

describe("panFraction", () => {
  it("maps the pane proportionally with reachable corners", () => {
    // Inside the 6% edge margin already reads as fully-there: the corner of
    // the image must not demand the corner PIXEL of the pane.
    expect(panFraction(0, 1000)).toBe(0);
    expect(panFraction(30, 1000)).toBe(0);
    expect(panFraction(500, 1000)).toBeCloseTo(0.5);
    expect(panFraction(970, 1000)).toBe(1);
    expect(panFraction(1000, 1000)).toBe(1);
  });

  it("never leaves [0, 1] even for coordinates outside the pane", () => {
    expect(panFraction(-50, 1000)).toBe(0);
    expect(panFraction(1200, 1000)).toBe(1);
  });

  it("degrades safely on a degenerate pane", () => {
    expect(panFraction(10, 0)).toBe(0);
  });
});

describe("intrinsicOffset", () => {
  it("centres a source smaller than the viewport", () => {
    expect(intrinsicOffset(400, 1000, 0)).toBe(300);
    expect(intrinsicOffset(400, 1000, 1)).toBe(300);
  });

  it("maps both edges of a source larger than the viewport", () => {
    expect(intrinsicOffset(4000, 1000, 0)).toBe(-0);
    expect(intrinsicOffset(4000, 1000, 0.5)).toBe(-1500);
    expect(intrinsicOffset(4000, 1000, 1)).toBe(-3000);
  });
});

// The 100% view's position-mapped panning (developer, 2026-08-17): the cursor
// position IN the pane is the position IN the image. Replaced drag-panning —
// crossing a mostly-hidden original took several grab strokes; this is one
// sweep of the mouse.

import { describe, expect, it } from "vitest";
import { panFraction } from "../../src/components/ZoomableImage";

describe("panFraction", () => {
  it("maps the pane proportionally with reachable corners", () => {
    // Inside the 6% edge margin already reads as fully-there: the corner of
    // the image must not demand the corner PIXEL of the pane.
    expect(panFraction(0, 1000)).toBe(0);
    expect(panFraction(30, 1000)).toBe(0); // still inside the margin
    expect(panFraction(500, 1000)).toBeCloseTo(0.5);
    expect(panFraction(970, 1000)).toBe(1);
    expect(panFraction(1000, 1000)).toBe(1);
  });

  it("never leaves [0, 1] even for coordinates outside the pane", () => {
    expect(panFraction(-50, 1000)).toBe(0);
    expect(panFraction(1200, 1000)).toBe(1);
  });

  it("degrades to centered on a degenerate pane", () => {
    expect(panFraction(10, 0)).toBe(0);
  });
});

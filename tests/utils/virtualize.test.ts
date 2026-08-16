import { describe, expect, it } from "vitest";
import { scrollTopForRow, visibleWindow } from "../../src/utils/virtualize";

describe("the visible window", () => {
  it("renders only the viewport's rows plus overscan", () => {
    // 10,000 rows of 190px, viewport 800px, scrolled deep: the DOM carries a
    // handful of rows, never ten thousand.
    const win = visibleWindow(950_000, 800, 190, 10_000);
    expect(win.startRow).toBe(4998); // floor(950000/190)=5000, minus overscan
    expect(win.endRow - win.startRow).toBeLessThanOrEqual(10);
    // The spacers stand in for everything else, so the scrollbar stays honest.
    expect(win.topPad).toBe(win.startRow * 190);
    expect(win.topPad + (win.endRow - win.startRow) * 190 + win.bottomPad).toBe(
      10_000 * 190,
    );
  });

  it("clamps at both ends", () => {
    const top = visibleWindow(0, 800, 190, 100);
    expect(top.startRow).toBe(0);
    expect(top.topPad).toBe(0);

    const bottom = visibleWindow(100 * 190, 800, 190, 100);
    expect(bottom.endRow).toBe(100);
    expect(bottom.bottomPad).toBe(0);
  });

  it("a small section renders whole", () => {
    const win = visibleWindow(0, 800, 190, 3);
    expect(win.startRow).toBe(0);
    expect(win.endRow).toBe(3);
    expect(win.topPad).toBe(0);
    expect(win.bottomPad).toBe(0);
  });

  it("survives the degenerate inputs", () => {
    expect(visibleWindow(0, 800, 190, 0)).toEqual({
      startRow: 0,
      endRow: 0,
      topPad: 0,
      bottomPad: 0,
    });
    expect(visibleWindow(0, 800, 0, 100).endRow).toBe(0);
  });
});

describe("the anchor-recovery scroll", () => {
  it("centres the row when the viewport has room", () => {
    // Row 50 of 190px in an 800px viewport: the row sits mid-viewport, so
    // the post-delete recovery lands where the user is looking.
    expect(scrollTopForRow(50, 800, 190)).toBe(50 * 190 - (800 - 190) / 2);
  });

  it("never scrolls above the top", () => {
    expect(scrollTopForRow(0, 800, 190)).toBe(0);
  });
});

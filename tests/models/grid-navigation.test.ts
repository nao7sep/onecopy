import { describe, expect, it } from "vitest";
import { gridNavigationTarget, pageRows } from "../../src/models/gridNavigation";

const base = {
  layout: "tiles" as const,
  columns: 4,
  viewportHeight: 600,
  rowHeight: 150,
  current: 10,
  total: 100,
};

describe("grid keyboard navigation", () => {
  it("pages by the rows the measured row height fits in the viewport (R7-12)", () => {
    expect(pageRows(600, 150)).toBe(4);
    expect(gridNavigationTarget({ ...base, key: "PageDown" })).toBe(10 + 4 * 4);
    // A larger zoom makes rows taller, so a page moves fewer rows.
    expect(gridNavigationTarget({ ...base, key: "PageDown", rowHeight: 290 })).toBe(10 + 2 * 4);
    expect(gridNavigationTarget({ ...base, key: "PageUp" })).toBe(0);
  });

  it("moves by one tile, one row, or to either end, clamped to the section", () => {
    expect(gridNavigationTarget({ ...base, key: "ArrowRight" })).toBe(11);
    expect(gridNavigationTarget({ ...base, key: "ArrowLeft" })).toBe(9);
    expect(gridNavigationTarget({ ...base, key: "ArrowDown" })).toBe(14);
    expect(gridNavigationTarget({ ...base, key: "ArrowUp" })).toBe(6);
    expect(gridNavigationTarget({ ...base, key: "Home" })).toBe(0);
    expect(gridNavigationTarget({ ...base, key: "End" })).toBe(99);
    expect(gridNavigationTarget({ ...base, key: "ArrowDown", current: 98 })).toBe(99);
    expect(gridNavigationTarget({ ...base, key: "ArrowDown", current: -1 })).toBe(0);
  });

  it("leaves other keys alone, lists move only vertically, and an empty section has no target", () => {
    expect(gridNavigationTarget({ ...base, key: "x" })).toBeNull();
    expect(gridNavigationTarget({ ...base, layout: "list", columns: 1, key: "ArrowRight" })).toBeNull();
    expect(gridNavigationTarget({ ...base, key: "End", total: 0 })).toBe(-1);
  });
});

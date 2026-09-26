import { describe, expect, it } from "vitest";
import {
  COMPARISON_DIRECT_KEYS,
  chunkMembers,
  comparisonPages,
  comparisonDisplayLayout,
  directKeyIndex,
  displayCapacities,
  gridFor,
  spatialTarget,
  updateComparisonDraft,
  type ComparisonDecisionDraft,
  type ComparisonMember,
} from "../../src/models/comparisonSession";

function member(index: number, portrait = false): ComparisonMember {
  return {
    hash: `h${index}`,
    fileName: `image-${index}.jpg`,
    width: portrait ? 3000 : 4000,
    height: portrait ? 4000 : 3000,
    byteSize: 1000,
    sharpness: null,
    faceScore: null,
    copyCount: 1,
    hasThumb: true,
  };
}

describe("comparison capacity", () => {
  it("limits a visible page without limiting the group", () => {
    const pages = comparisonPages(
      Array.from({ length: 41 }, (_, index) => member(index)),
      16,
      Array.from({ length: 10 }, () => 16 / 9),
    );
    expect(pages.map((page) => page.members.length)).toEqual([16, 16, 9]);
    expect(pages.flatMap((page) => page.members)).toHaveLength(41);
  });

  it("derives each display capacity from image and display orientation", () => {
    expect(comparisonDisplayLayout(false, 16 / 9)).toEqual({
      capacity: 4, columns: 2, rows: 2,
    });
    expect(comparisonDisplayLayout(false, 9 / 16)).toEqual({
      capacity: 3, columns: 1, rows: 3,
    });
    expect(comparisonDisplayLayout(true, 16 / 9)).toEqual({
      capacity: 3, columns: 3, rows: 1,
    });
    expect(comparisonDisplayLayout(true, 9 / 16)).toEqual({
      capacity: 4, columns: 2, rows: 2,
    });
    expect(displayCapacities(10, [4, 3, 4, 3])).toEqual([4, 3, 4]);
  });

  it("uses heterogeneous capacities to choose page boundaries", () => {
    const wide = comparisonPages(
      Array.from({ length: 15 }, (_, index) => member(index)),
      16,
      [16 / 9, 9 / 16],
    );
    expect(wide.map((page) => page.members.length)).toEqual([7, 7, 1]);
    expect(wide[0]?.capacities).toEqual([4, 3]);

    const tall = comparisonPages(
      Array.from({ length: 15 }, (_, index) => member(index, true)),
      16,
      [16 / 9, 9 / 16],
    );
    expect(tall.map((page) => page.members.length)).toEqual([7, 7, 1]);
    expect(tall[0]?.capacities).toEqual([3, 4]);
  });

  it("resolves a page that ties once its own capacity shrinks it to landscape", () => {
    // Two landscape displays, 16 members allowed: the first 8 members are
    // strictly portrait-dominant (5 portrait, 3 landscape), so the model
    // narrows to the portrait capacity (3 per display, 6 members). That
    // narrower slice is itself an exact 3-3 tie, which the spec resolves to
    // landscape, so the page must keep those 6 members but present them
    // with the landscape (2x2) capacities rather than relabeling them
    // portrait or growing back to the wider, portrait-dominant slice.
    const members = [
      member(0),
      member(1),
      member(2),
      member(3, true),
      member(4, true),
      member(5, true),
      member(6, true),
      member(7, true),
    ];
    const pages = comparisonPages(members, 16, [16 / 9, 16 / 9]);
    expect(pages[0]?.members).toHaveLength(6);
    expect(pages[0]?.portraitDominant).toBe(false);
    expect(pages[0]?.capacities).toEqual([4, 4]);
    expect(pages.flatMap((page) => page.members)).toHaveLength(8);
  });

  it("does not count unknown dimensions as landscape votes", () => {
    const unknown = { ...member(2), width: null, height: null };
    expect(
      comparisonPages([member(0, true), unknown], 16, [16 / 9])[0]?.capacities,
    ).toEqual([3]);
  });

  it("chunks in configured display order", () => {
    expect(chunkMembers([0, 1, 2, 3, 4, 5], [4, 4])).toEqual([
      [0, 1, 2, 3],
      [4, 5],
    ]);
  });
});

describe("comparison card order and navigation", () => {
  it("flows landscape cards left-to-right then top-to-bottom", () => {
    expect(gridFor(4, false)).toEqual({ count: 4, columns: 2, rows: 2 });
    expect(spatialTarget(0, "down", [4], false)).toBe(2);
    expect(spatialTarget(0, "right", [4], false)).toBe(1);
  });

  it("adapts the grid to a portrait display", () => {
    expect(gridFor(3, false, 9 / 16, 3)).toEqual({
      count: 3,
      columns: 1,
      rows: 3,
    });
    expect(gridFor(4, true, 9 / 16, 4)).toEqual({
      count: 4,
      columns: 2,
      rows: 2,
    });
  });

  it("keeps a partial final display on the full capacity grid", () => {
    expect(gridFor(1, false, 16 / 9, 4)).toEqual({
      count: 1,
      columns: 2,
      rows: 2,
    });
    expect(gridFor(1, true, 16 / 9, 3)).toEqual({
      count: 1,
      columns: 3,
      rows: 1,
    });
  });

  it("crosses display edges without wrapping the outer bounds", () => {
    expect(spatialTarget(1, "right", [4, 4], false)).toBe(4);
    expect(spatialTarget(3, "right", [4, 4], false)).toBe(6);
    expect(spatialTarget(4, "left", [4, 4], false)).toBe(1);
    expect(spatialTarget(0, "left", [4, 4], false)).toBe(0);
  });

  it("matches partial rows and differently shaped neighboring displays", () => {
    expect(
      spatialTarget(3, "right", [4, 3], false, [16 / 9, 9 / 16], [4, 3]),
    ).toBe(5);
    expect(
      spatialTarget(6, "left", [4, 3], false, [16 / 9, 9 / 16], [4, 3]),
    ).toBe(3);
    expect(spatialTarget(2, "down", [3], false, [9 / 16], [3])).toBe(2);
  });
});

describe("direct image keys", () => {
  it("assigns 0-9 then A-Z and leaves later cards unassigned", () => {
    expect(COMPARISON_DIRECT_KEYS).toHaveLength(36);
    expect(COMPARISON_DIRECT_KEYS.slice(0, 11)).toEqual([
      "0",
      "1",
      "2",
      "3",
      "4",
      "5",
      "6",
      "7",
      "8",
      "9",
      "a",
    ]);
  });

  it("accepts only a bare non-repeating assigned key", () => {
    expect(directKeyIndex({ key: "0" })).toBe(0);
    expect(directKeyIndex({ key: "F" })).toBe(15);
    expect(directKeyIndex({ key: "z" })).toBe(35);
    expect(directKeyIndex({ key: "f", shiftKey: true })).toBe(-1);
    expect(directKeyIndex({ key: "f", ctrlKey: true })).toBe(-1);
    expect(directKeyIndex({ key: "f", repeat: true })).toBe(-1);
  });
});

describe("comparison range selection with no origin", () => {
  it("keeps existing marks when a Shift range starts with no remembered origin", () => {
    // A repagination (e.g. the anchor's member left the page) can leave
    // rangeOrigin and anchor both null while marks made on the page are
    // still visible. A fresh Shift press must extend from those marks,
    // not replace them with a single-image selection.
    const members = [member(0), member(1), member(2), member(3)];
    const draft: ComparisonDecisionDraft = {
      selected: new Set(["h1"]),
      anchors: new Set(),
      anchor: null,
      rangeOrigin: null,
      rangeBase: new Set(),
    };
    const result = updateComparisonDraft(draft, members, "h2", "range");
    expect(result.selected).toEqual(new Set(["h1", "h2"]));
    expect(result.rangeOrigin).toBe("h2");
    expect(result.rangeBase).toEqual(new Set(["h1"]));
  });
});

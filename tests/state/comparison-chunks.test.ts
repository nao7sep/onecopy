import { describe, expect, it } from "vitest";
import {
  COMPARISON_DIRECT_KEYS,
  chunkMembers,
  comparisonPages,
  directKeyIndex,
  displayCapacities,
  gridFor,
  spatialTarget,
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
      10,
    );
    expect(pages.map((page) => page.members.length)).toEqual([16, 16, 9]);
    expect(pages.flatMap((page) => page.members)).toHaveLength(41);
  });

  it("uses three portrait cards or four landscape cards per display", () => {
    expect(comparisonPages([member(0), member(1)], 16, 1)[0]?.perDisplay).toBe(
      4,
    );
    expect(
      comparisonPages([member(0, true), member(1, true)], 16, 1)[0]?.perDisplay,
    ).toBe(3);
    expect(displayCapacities(10, 4, 8)).toEqual([4, 4, 4]);
  });

  it("does not count unknown dimensions as landscape votes", () => {
    const unknown = { ...member(2), width: null, height: null };
    expect(
      comparisonPages([member(0, true), unknown], 16, 1)[0]?.perDisplay,
    ).toBe(3);
  });

  it("chunks in configured display order", () => {
    expect(chunkMembers([0, 1, 2, 3, 4, 5], [4, 4])).toEqual([
      [0, 1, 2, 3],
      [4, 5],
    ]);
  });
});

describe("comparison card order and navigation", () => {
  it("flows landscape cards top-to-bottom then left-to-right", () => {
    expect(gridFor(4, false)).toEqual({ count: 4, columns: 2, rows: 2 });
    expect(spatialTarget(0, "down", [4], false)).toBe(1);
    expect(spatialTarget(0, "right", [4], false)).toBe(2);
  });

  it("adapts the grid to a portrait display", () => {
    expect(gridFor(4, false, 9 / 16)).toEqual({
      count: 4,
      columns: 1,
      rows: 4,
    });
    expect(gridFor(3, true, 9 / 16)).toEqual({
      count: 3,
      columns: 2,
      rows: 2,
    });
  });

  it("crosses display edges without wrapping the outer bounds", () => {
    expect(spatialTarget(2, "right", [4, 4], false)).toBe(4);
    expect(spatialTarget(4, "left", [4, 4], false)).toBe(2);
    expect(spatialTarget(0, "left", [4, 4], false)).toBe(0);
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

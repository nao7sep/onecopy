import { describe, expect, it } from "vitest";
import { extLabel, sortItems, type SectionItem } from "../../src/models/items";

function item(overrides: Partial<SectionItem>): SectionItem {
  return {
    hash: null,
    pathId: 0,
    fileName: "f",
    resolvedUtcMs: null,
    copyCount: 1,
    width: null,
    height: null,
    hasThumb: false,
    similarGroupId: null,
    sharpness: null,
    byteSize: null,
    hasCompanions: false,
    durationMs: null,
    ...overrides,
  };
}

describe("sortItems", () => {
  const a = item({ pathId: 1, fileName: "b.jpg", resolvedUtcMs: 200, byteSize: 10, width: 100, height: 100 });
  const b = item({ pathId: 2, fileName: "A.jpg", resolvedUtcMs: 100, byteSize: 30, width: 200, height: 200 });
  const undatedItem = item({ pathId: 3, fileName: "c.jpg", resolvedUtcMs: null, byteSize: 20 });

  it("time puts oldest first and undated last", () => {
    expect(sortItems([a, b, undatedItem], "time").map((i) => i.pathId)).toEqual([2, 1, 3]);
  });

  it("name is case-insensitive", () => {
    expect(sortItems([a, b], "name").map((i) => i.fileName)).toEqual(["A.jpg", "b.jpg"]);
  });

  it("size puts largest first", () => {
    expect(sortItems([a, b, undatedItem], "size").map((i) => i.byteSize)).toEqual([30, 20, 10]);
  });

  it("resolution puts the biggest pixel count first", () => {
    expect(sortItems([a, b], "resolution").map((i) => i.pathId)).toEqual([2, 1]);
  });

  it("never mutates the input", () => {
    const input = [a, b];
    sortItems(input, "name");
    expect(input.map((i) => i.pathId)).toEqual([1, 2]);
  });
});

describe("extLabel", () => {
  it("uppercases extensions and falls back for none", () => {
    expect(extLabel("scan.pdf")).toBe("PDF");
    expect(extLabel("noext")).toBe("FILE");
  });
});

import { describe, expect, it } from "vitest";
import { DEFAULT_DESC, SORT_ORDERS, extLabel, extOf, sortItems, type SectionItem, type SortOrder } from "../../src/models/items";

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
    dirPaths: ["/files"],
    ...overrides,
  };
}

describe("sortItems", () => {
  const a = item({ pathId: 1, fileName: "b.jpg", resolvedUtcMs: 200, byteSize: 10, width: 100, height: 100 });
  const b = item({ pathId: 2, fileName: "A.jpg", resolvedUtcMs: 100, byteSize: 30, width: 200, height: 200 });
  const undatedItem = item({ pathId: 3, fileName: "c.jpg", resolvedUtcMs: null, byteSize: 20 });

  it("time puts oldest first and undated last", () => {
    expect(sortItems([a, b, undatedItem], { order: "time", desc: false }).map((i) => i.pathId)).toEqual([2, 1, 3]);
  });

  it("name is case-insensitive", () => {
    expect(sortItems([a, b], { order: "name", desc: false }).map((i) => i.fileName)).toEqual(["A.jpg", "b.jpg"]);
  });

  it("size puts largest first", () => {
    expect(sortItems([a, b, undatedItem], { order: "size", desc: true }).map((i) => i.byteSize)).toEqual([30, 20, 10]);
  });

  it("resolution puts the biggest pixel count first", () => {
    expect(sortItems([a, b], { order: "resolution", desc: true }).map((i) => i.pathId)).toEqual([2, 1]);
  });

  it("never mutates the input", () => {
    const input = [a, b];
    sortItems(input, { order: "name", desc: false });
    expect(input.map((i) => i.pathId)).toEqual([1, 2]);
  });
});

describe("extLabel", () => {
  it("uppercases extensions and falls back for none", () => {
    expect(extLabel("scan.pdf")).toBe("PDF");
    expect(extLabel("noext")).toBe("FILE");
  });
});

describe("the file-manager orders (other-files table)", () => {
  const doc = item({ pathId: 1, fileName: "notes.TXT", dirPaths: ["/docs"] });
  const zip = item({ pathId: 2, fileName: "backup.zip", dirPaths: ["/archive"] });
  const doc2 = item({ pathId: 3, fileName: "agenda.txt", dirPaths: ["/docs"] });

  it("extOf lowercases and treats a dotless or dotfile name as extensionless", () => {
    expect(extOf("notes.TXT")).toBe("txt");
    expect(extOf("Makefile")).toBe("");
    expect(extOf(".gitignore")).toBe("");
  });

  it("ext groups by extension with name breaking ties", () => {
    expect(sortItems([doc, zip, doc2], { order: "ext", desc: false }).map((i) => i.fileName)).toEqual([
      "agenda.txt",
      "notes.TXT",
      "backup.zip",
    ]);
  });

  it("offers NO folder sort — copies merge into one row with many folders", () => {
    // Any single folder key was an arbitrary MIN over the merged copies
    // (Phase 33): the Folders column displays them all and never sorts.
    expect(Object.keys(SORT_ORDERS.other.orders)).not.toContain("folder");
  });
});

describe("the per-kind sort catalogues", () => {
  it("offer only orders the kind can honour", () => {
    // "Time taken" over files nobody took, and "Resolution" over files with
    // no pixels, is the exact mislabeling this split exists to end.
    expect(Object.keys(SORT_ORDERS.other.orders)).not.toContain("resolution");
    expect(SORT_ORDERS.other.orders.time).toBe("Date");
    expect(SORT_ORDERS.media.orders.time).toBe("Time taken");
  });

  it("default each kind the way its file manager would", () => {
    expect(SORT_ORDERS.media.defaultChoice).toEqual({ order: "time", desc: false });
    expect(SORT_ORDERS.other.defaultChoice).toEqual({ order: "name", desc: false });
  });

  it("every offered order is implemented — no menu entry can no-op", () => {
    const items = [doc3(), doc3()];
    for (const catalogue of Object.values(SORT_ORDERS)) {
      for (const order of Object.keys(catalogue.orders) as SortOrder[]) {
        // A missing switch case would return the input untouched — same
        // array contents is fine, but the CALL must not throw.
        expect(() => sortItems(items, { order, desc: DEFAULT_DESC[order] })).not.toThrow();
      }
    }
  });

  function doc3() {
    return item({ pathId: Math.floor(1), fileName: "x.txt" });
  }
});

describe("directions and tie-break chains (Phase 33)", () => {
  const shot = (pathId: number, over: Partial<SectionItem>) => item({ pathId, ...over });

  it("desc flips the primary key only — ties stay in shooting order", () => {
    // Three same-resolution phone shots and one small export: resolution
    // descending must show the big group FIRST but INSIDE it oldest-first,
    // the way Finder reads a descending sort.
    const a = shot(1, { width: 4000, height: 3000, resolvedUtcMs: 3000, fileName: "c.jpg" });
    const b = shot(2, { width: 4000, height: 3000, resolvedUtcMs: 1000, fileName: "a.jpg" });
    const c = shot(3, { width: 4000, height: 3000, resolvedUtcMs: 2000, fileName: "b.jpg" });
    const small = shot(4, { width: 100, height: 100, resolvedUtcMs: 500 });
    const sorted = sortItems([a, small, b, c], { order: "resolution", desc: true });
    expect(sorted.map((i) => i.pathId)).toEqual([2, 3, 1, 4]);
  });

  it("every order is total: identical items fall back to pathId", () => {
    const twin = (pathId: number) =>
      shot(pathId, { fileName: "same.jpg", resolvedUtcMs: 1000, byteSize: 5 });
    const sorted = sortItems([twin(9), twin(3), twin(7)], { order: "name", desc: false });
    expect(sorted.map((i) => i.pathId)).toEqual([3, 7, 9]);
  });

  it("time ascending and descending are exact mirrors on distinct keys", () => {
    const items = [1000, 3000, 2000].map((t, i) => shot(i + 1, { resolvedUtcMs: t }));
    const asc = sortItems(items, { order: "time", desc: false }).map((i) => i.pathId);
    const desc = sortItems(items, { order: "time", desc: true }).map((i) => i.pathId);
    expect(desc).toEqual([...asc].reverse());
  });
});

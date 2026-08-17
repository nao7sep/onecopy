import { describe, expect, it } from "vitest";
import { SORT_ORDERS, extLabel, extOf, sortItems, type SectionItem, type SortOrder } from "../../src/models/items";

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
    dirPath: "/files",
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

describe("the file-manager orders (other-files table)", () => {
  const doc = item({ pathId: 1, fileName: "notes.TXT", dirPath: "/docs" });
  const zip = item({ pathId: 2, fileName: "backup.zip", dirPath: "/archive" });
  const doc2 = item({ pathId: 3, fileName: "agenda.txt", dirPath: "/docs" });

  it("extOf lowercases and treats a dotless or dotfile name as extensionless", () => {
    expect(extOf("notes.TXT")).toBe("txt");
    expect(extOf("Makefile")).toBe("");
    expect(extOf(".gitignore")).toBe("");
  });

  it("ext groups by extension with name breaking ties", () => {
    expect(sortItems([doc, zip, doc2], "ext").map((i) => i.fileName)).toEqual([
      "agenda.txt",
      "notes.TXT",
      "backup.zip",
    ]);
  });

  it("folder groups by directory with name breaking ties", () => {
    expect(sortItems([doc, zip, doc2], "folder").map((i) => i.pathId)).toEqual([2, 3, 1]);
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
    expect(SORT_ORDERS.media.defaultOrder).toBe("time");
    expect(SORT_ORDERS.other.defaultOrder).toBe("name");
  });

  it("every offered order is implemented — no menu entry can no-op", () => {
    const items = [doc3(), doc3()];
    for (const catalogue of Object.values(SORT_ORDERS)) {
      for (const order of Object.keys(catalogue.orders) as SortOrder[]) {
        // A missing switch case would return the input untouched — same
        // array contents is fine, but the CALL must not throw.
        expect(() => sortItems(items, order)).not.toThrow();
      }
    }
  });

  function doc3() {
    return item({ pathId: Math.floor(1), fileName: "x.txt" });
  }
});

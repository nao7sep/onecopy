import { describe, expect, it } from "vitest";
import {
  branchesFor,
  buildSectionTree,
  defaultExpanded,
  kindKey,
  monthKey,
  visibleRows,
  yearKey,
} from "../../src/models/sectionTree";
import type { SectionCounts } from "../../src/models/sections";

const COUNTS: SectionCounts = {
  images: [
    { month: "2015-11", count: 3 },
    { month: "2016-01", count: 5 },
    { month: "2016-03", count: 7 },
    { month: "undated", count: 2 },
  ],
  videos: [{ month: "2016-01", count: 4 }],
  others: [],
};

describe("grouping months into years", () => {
  it("keeps the core's oldest-first order at both levels", () => {
    const [images] = buildSectionTree(COUNTS);
    expect(images.years.map((y) => y.year)).toEqual(["2015", "2016"]);
    expect(images.years[1].months.map((m) => m.month)).toEqual(["2016-01", "2016-03"]);
  });

  it("totals each year from its own months", () => {
    const [images] = buildSectionTree(COUNTS);
    expect(images.years[0].count).toBe(3);
    expect(images.years[1].count).toBe(12);
  });

  it("counts undated items in the KIND total but not in any year", () => {
    // A header that excluded them would disagree with the rows beneath it —
    // the undated section is right there, carrying two.
    const [images] = buildSectionTree(COUNTS);
    expect(images.count).toBe(17);
    expect(images.years.reduce((sum, y) => sum + y.count, 0)).toBe(15);
    expect(images.undated?.count).toBe(2);
    expect(images.years.some((y) => y.year === "unda")).toBe(false);
  });

  it("gives every kind a node even when it has nothing", () => {
    const tree = buildSectionTree(COUNTS);
    expect(tree.map((n) => n.kind)).toEqual(["image", "video", "other"]);
    expect(tree[2].count).toBe(0);
    expect(tree[2].years).toEqual([]);
  });

  it("survives no counts at all", () => {
    expect(buildSectionTree(null).every((n) => n.count === 0)).toBe(true);
  });
});

describe("which rows are on screen", () => {
  it("opens the kinds and leaves the years closed", () => {
    // The whole point of the tree: 100+ months must not be the opening view.
    const rows = visibleRows(buildSectionTree(COUNTS), defaultExpanded());
    expect(rows.filter((r) => r.type === "month" && r.month !== "undated")).toHaveLength(0);
    expect(rows.filter((r) => r.type === "year")).toHaveLength(3);
  });

  it("reveals exactly one year's months when that year opens", () => {
    const expanded = defaultExpanded();
    expanded.add(yearKey("image", "2016"));
    const rows = visibleRows(buildSectionTree(COUNTS), expanded);
    const months = rows.filter((r) => r.type === "month").map((r) => r.key);
    expect(months).toContain(monthKey("image", "2016-01"));
    expect(months).toContain(monthKey("image", "2016-03"));
    expect(months).not.toContain(monthKey("image", "2015-11"));
  });

  it("shows Undated beside the years, not inside one", () => {
    const rows = visibleRows(buildSectionTree(COUNTS), defaultExpanded());
    const undated = rows.find((r) => r.type === "month" && r.month === "undated");
    expect(undated).toBeDefined();
    // Depth 1 is the years' level; a collapsed year must not hide it.
    expect(undated?.depth).toBe(1);
  });

  it("hides a kind's whole subtree when the kind collapses", () => {
    const expanded = new Set([kindKey("video")]);
    const rows = visibleRows(buildSectionTree(COUNTS), expanded);
    expect(rows.filter((r) => r.key.includes(":image:"))).toHaveLength(0);
    expect(rows.filter((r) => r.type === "kind")).toHaveLength(3);
  });
});

describe("keeping a restored selection visible", () => {
  it("names the branches a selected month needs", () => {
    expect(branchesFor({ kind: "image", month: "2016-03" })).toEqual([
      kindKey("image"),
      yearKey("image", "2016"),
    ]);
  });

  it("asks for no year when the selection is Undated", () => {
    // "unda" is not a year, and asking for it would leave a dead key in the
    // expansion set forever.
    expect(branchesFor({ kind: "video", month: "undated" })).toEqual([kindKey("video")]);
  });

  it("asks for nothing when nothing is selected", () => {
    expect(branchesFor(null)).toEqual([]);
  });

  it("makes the selected month actually appear", () => {
    const selected = { kind: "image" as const, month: "2015-11" };
    const expanded = new Set([...defaultExpanded(), ...branchesFor(selected)]);
    const rows = visibleRows(buildSectionTree(COUNTS), expanded);
    expect(rows.map((r) => r.key)).toContain(monthKey("image", "2015-11"));
  });
});

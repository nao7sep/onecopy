import { describe, expect, it } from "vitest";
import { resolveWorkAttention, viewportAttention, type WorkAttention } from "../../src/models/workAttention";

const main: WorkAttention = {
  selectedHash: "image", visibleHashes: ["image", "next"], nearbyHashes: ["near"],
  sectionKind: "image", sectionMonth: "2026-09", sectionSort: { order: "time", desc: false },
  sectionAnchor: 200, sectionTotal: 10000,
};

describe("work attention", () => {
  it("orders nearby targets by viewport distance and caps the neighborhood", () => {
    const hashes = Array.from({ length: 40 }, (_, i) => String(i));
    const view = viewportAttention(hashes, 100, 115, 118);
    expect(view.visibleHashes).toEqual(["15", "16", "17"]);
    expect(view.nearbyHashes.slice(0, 6)).toEqual(["14", "18", "13", "19", "12", "20"]);
    expect(view.nearbyHashes).toHaveLength(16);
    expect(view.anchor).toBe(115);
  });

  it("comparison replaces hidden image or video section work with the whole visible page", () => {
    const view = resolveWorkAttention(main, { selected: "card-3", visible: ["card-1", "card-2", "card-3"] }, null);
    expect(view.selectedHash).toBe("card-3");
    expect(view.visibleHashes).toEqual(["card-1", "card-2", "card-3"]);
    expect(view.nearbyHashes).toEqual([]);
    expect(view.sectionKind).toBeNull();
    expect(view.sectionTotal).toBe(0);
    expect(resolveWorkAttention(main, null, null)).toEqual(main);
  });

  it("a distant Quick View move replaces the old viewport while Main catches up", () => {
    const view = resolveWorkAttention(main, null, { hash: "far", sectionIndex: 900 });
    expect(view.visibleHashes).toEqual(["far"]);
    expect(view.nearbyHashes).toEqual([]);
    expect(view.sectionAnchor).toBe(900);
  });

  it("Other attention retains audio targets and null identities never enter derived work", () => {
    const view = viewportAttention([null, "audio", null, "document"], 0, 0, 3);
    expect(view.visibleHashes).toEqual(["audio"]);
    expect(resolveWorkAttention({ ...main, sectionKind: "other", ...view, sectionAnchor: view.anchor }, null, null).sectionKind).toBe("other");
  });
});

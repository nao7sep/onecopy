import { describe, expect, it } from "vitest";
import { chunkSlots, type GroupMember } from "../../src/state/comparison-store";

function member(hash: string): GroupMember {
  return {
    hash,
    fileName: `${hash}.jpg`,
    width: null,
    height: null,
    sharpness: null,
    copyCount: 1,
    hasThumb: true,
  };
}

describe("chunkSlots", () => {
  const eight = ["a", "b", "c", "d", "e", "f", "g", "h"].map(member);

  it("keeps one global key space across screens", () => {
    const chunks = chunkSlots(eight, new Set(), 3);
    expect(chunks).toHaveLength(3);
    expect(chunks[0].map((s) => s.slotKey)).toEqual(["1", "2", "3"]);
    expect(chunks[1].map((s) => s.slotKey)).toEqual(["4", "5", "6"]);
    expect(chunks[2].map((s) => s.slotKey)).toEqual(["7", "8"]);
  });

  it("single screen holds everything", () => {
    const chunks = chunkSlots(eight, new Set(), 1);
    expect(chunks).toHaveLength(1);
    expect(chunks[0]).toHaveLength(8);
  });

  it("marks keepers inside the chunks", () => {
    const chunks = chunkSlots(eight, new Set(["d"]), 2);
    const flat = chunks.flat();
    expect(flat.find((s) => s.member.hash === "d")?.kept).toBe(true);
    expect(flat.filter((s) => s.kept)).toHaveLength(1);
  });

  it("letters appear past slot ten", () => {
    const many = Array.from({ length: 12 }, (_, i) => member(`m${i}`));
    const chunks = chunkSlots(many, new Set(), 1);
    expect(chunks[0][9].slotKey).toBe("0");
    expect(chunks[0][10].slotKey).toBe("a");
    expect(chunks[0][11].slotKey).toBe("b");
  });
});

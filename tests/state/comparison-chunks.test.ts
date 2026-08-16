import { describe, expect, it } from "vitest";
import {
  chunkSlots,
  perScreenCapacity,
  turnSize,
  type GroupMember,
} from "../../src/state/comparison-store";

function member(hash: string): GroupMember {
  return {
    hash,
    fileName: `${hash}.jpg`,
    width: null,
    height: null,
  byteSize: null,
    sharpness: null,
    copyCount: 1,
    hasThumb: true,
  };
}

describe("chunkSlots", () => {
  const eight = ["a", "b", "c", "d", "e", "f", "g", "h"].map(member);

  it("keeps one global key space across capacity-weighted screens", () => {
    // The developer's three-screen layout: a portrait-ish top screen (3) and
    // two landscape screens (4 each) — but with 8 members the fill is 3/4/1.
    const chunks = chunkSlots(eight, new Set(), [3, 4, 4]);
    expect(chunks).toHaveLength(3);
    expect(chunks[0].map((s) => s.slotKey)).toEqual(["1", "2", "3"]);
    expect(chunks[1].map((s) => s.slotKey)).toEqual(["4", "5", "6", "7"]);
    expect(chunks[2].map((s) => s.slotKey)).toEqual(["8"]);
  });

  it("single screen holds everything", () => {
    const chunks = chunkSlots(eight, new Set(), [16]);
    expect(chunks).toHaveLength(1);
    expect(chunks[0]).toHaveLength(8);
  });

  it("marks keepers inside the chunks", () => {
    const chunks = chunkSlots(eight, new Set(["d"]), [4, 4]);
    const flat = chunks.flat();
    expect(flat.find((s) => s.member.hash === "d")?.kept).toBe(true);
    expect(flat.filter((s) => s.kept)).toHaveLength(1);
  });

  it("letters appear past slot ten", () => {
    const many = Array.from({ length: 12 }, (_, i) => member(`m${i}`));
    const chunks = chunkSlots(many, new Set(), [16]);
    expect(chunks[0][9].slotKey).toBe("0");
    expect(chunks[0][10].slotKey).toBe("a");
    expect(chunks[0][11].slotKey).toBe("b");
  });

  it("turn size is the capacity sum capped by the sixteen keys", () => {
    expect(turnSize([3, 4, 4])).toBe(11);
    expect(turnSize([16])).toBe(16);
    expect(turnSize([9, 9])).toBe(16);
    expect(turnSize([])).toBe(1);
  });

  it("chunking and turn size agree for the same capacities", () => {
    // These were asserted in isolation, so the fact that they DISAGREE for []
    // — turnSize returns 1 while chunkSlots falls back to [slots.length] —
    // could not be seen. Unreachable today (capacities initialises to [16]),
    // so this pins a latent trap rather than a live bug.
    const slots = Array.from({ length: 20 }, (_, i) => member(`h${i}`));
    for (const capacities of [[], [4], [4, 4], [16], [3, 4, 4]]) {
      const size = turnSize(capacities);
      const chunked = chunkSlots(slots.slice(0, size), new Set(), capacities);
      expect(chunked.flat()).toHaveLength(size);
    }
  });

  it("per-screen capacity follows the group's dominant image orientation", () => {
    const portrait = (h: string) => ({ ...member(h), width: 3000, height: 4000 });
    const landscape = (h: string) => ({ ...member(h), width: 4000, height: 3000 });
    expect(perScreenCapacity([portrait("a"), portrait("b"), landscape("c")])).toBe(3);
    expect(perScreenCapacity([portrait("a"), landscape("b"), landscape("c")])).toBe(4);
    // Unknown dimensions count as landscape (the roomier default).
    expect(perScreenCapacity([member("a"), member("b")])).toBe(4);
  });
});

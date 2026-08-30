import { describe, expect, it } from "vitest";
import {
  anchorContext,
  parseAnchorContext,
  recoverAnchor,
} from "../../src/models/mainSelection";

describe("main work-position recovery", () => {
  it("chooses the next remembered survivor, then the previous one", () => {
    const order = ["a", "b", "c", "d", "e"];
    const context = anchorContext(order, "c");
    expect(recoverAnchor(["a", "b", "d", "e"], "c", context)).toBe("d");
    expect(recoverAnchor(["a", "b"], "c", context)).toBe("b");
    expect(recoverAnchor([], "c", context)).toBeNull();
    expect(recoverAnchor(["a"], null, null)).toBeNull();
  });

  it("prefers a selected survivor during multi-selection recovery", () => {
    const context = anchorContext(["a", "b", "c", "d", "e"], "c");
    expect(
      recoverAnchor(["a", "b", "d", "e"], "c", context, new Set(["b", "e"])),
    ).toBe("e");
  });

  it("accepts only bounded well-formed persisted context", () => {
    expect(parseAnchorContext({ index: 3, before: ["b"], after: ["d"] })).toEqual({
      index: 3,
      before: ["b"],
      after: ["d"],
    });
    expect(parseAnchorContext({ index: -1, before: [], after: [] })).toBeNull();
    expect(parseAnchorContext({ index: 1, before: [2], after: [] })).toBeNull();
  });
});

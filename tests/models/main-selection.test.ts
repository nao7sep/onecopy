import { describe, expect, it } from "vitest";
import { parseAnchorContext } from "../../src/models/mainSelection";

describe("main work-position recovery", () => {
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

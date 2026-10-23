import { describe, expect, it } from "vitest";
import { anchorContextPayload, parseAnchorContext } from "../../src/models/mainSelection";

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

it.each(["before", "after"])("drops optional context with an invalid %s identity", (side) => {
  for (const key of ["path-invalid", "path--1", "path-1.5", "path-Infinity", "path-9007199254740992"]) {
    expect(parseAnchorContext({ index: 3, before: [], after: [], [side]: [key] })).toBeNull();
  }
});

it("retains bounded hashed and unhashed neighbors that convert to native identities", () => {
  const context = parseAnchorContext({ index: 3, before: ["hash", "path-42"], after: Array(100).fill("path-7") });
  expect(context?.after).toHaveLength(64);
  expect(anchorContextPayload(context)?.before).toEqual([{ hash: "hash", pathId: 0 }, { hash: null, pathId: 42 }]);
  expect(anchorContextPayload(context)?.after).toHaveLength(64);
});

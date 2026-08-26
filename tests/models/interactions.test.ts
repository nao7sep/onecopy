import { describe, expect, it } from "vitest";
import { comparisonHashForEnter } from "../../src/models/interactions";

describe("main Enter routing", () => {
  it("opens only a hashed live similar family", () => {
    expect(comparisonHashForEnter({ hash: "h1", similarGroupId: 7 })).toBe("h1");
    expect(comparisonHashForEnter({ hash: "video", similarGroupId: null })).toBeNull();
    expect(comparisonHashForEnter({ hash: "plain-image", similarGroupId: null })).toBeNull();
    expect(comparisonHashForEnter({ hash: null, similarGroupId: 7 })).toBeNull();
    expect(comparisonHashForEnter(null)).toBeNull();
  });
});

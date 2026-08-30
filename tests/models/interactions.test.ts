import { describe, expect, it } from "vitest";
import { comparisonHashForSelection } from "../../src/models/interactions";

describe("main Enter routing", () => {
  it("opens only when the whole selection belongs to one live family", () => {
    const items = [
      { hash: "h1", similarGroupId: 7 },
      { hash: "h2", similarGroupId: 7 },
      { hash: "h3", similarGroupId: 8 },
      { hash: null, similarGroupId: 7 },
    ];
    expect(comparisonHashForSelection(items, new Set(["h1", "h2"]), "h2")).toBe("h2");
    expect(comparisonHashForSelection(items, new Set(["h1", "h3"]), "h3")).toBeNull();
    expect(comparisonHashForSelection(items, new Set(["h1", "path-4"]), "h1")).toBeNull();
    expect(comparisonHashForSelection(items, new Set(), null)).toBeNull();
  });
});

import { describe, expect, it } from "vitest";
import { monthLabel } from "../../src/models/sections";

describe("monthLabel", () => {
  it("passes real months through untouched", () => {
    expect(monthLabel("2016-03")).toBe("2016-03");
  });

  it("renders the undated sentinel as Undated", () => {
    expect(monthLabel("undated")).toBe("Undated");
  });
});

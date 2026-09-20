import { describe, expect, it } from "vitest";
import { monthLabel } from "../../src/models/sections";
import { t } from "../helpers/i18n";

describe("monthLabel", () => {
  it("leaves real months to name themselves", () => {
    // "2016-03" reads the same in every language, so there is nothing to
    // translate and the sidebar shows the month key as it stands.
    expect(monthLabel("2016-03")).toBeNull();
  });

  it("renders the undated sentinel as Undated", () => {
    const key = monthLabel("undated");
    expect(key).not.toBeNull();
    expect(t(key!)).toBe("Undated");
  });
});

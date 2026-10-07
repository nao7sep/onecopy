import { describe, expect, it } from "vitest";
import { configFlag, configNumber, confirmsTrashDelete } from "../../src/models/config";
import { effectiveConfig } from "../helpers/config";

describe("effective configuration readers", () => {
  it("confirm a single-item Delete as the core's defaults say, and honour opting out", () => {
    expect(confirmsTrashDelete(effectiveConfig())).toBe(true);
    expect(confirmsTrashDelete(effectiveConfig({ confirmTrashDelete: false }))).toBe(false);
  });

  it("supply no default of their own", () => {
    expect(configFlag({}, "autoplay")).toBe(false);
    expect(configNumber({}, "maximumImagesInComparison")).toBeNull();
    expect(configNumber({ maximumImagesInComparison: "16" }, "maximumImagesInComparison")).toBeNull();
  });
});

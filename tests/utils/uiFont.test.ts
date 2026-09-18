// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";

import { applyUiFont } from "../../src/utils/uiFont";

describe("UI font preference", () => {
  it("keeps CSS fallback ownership out of the user preference", () => {
    applyUiFont("Iosevka, monospace");
    expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe(
      "Iosevka, monospace",
    );

    applyUiFont(
      'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
    );
    expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe(
      "",
    );
  });
});

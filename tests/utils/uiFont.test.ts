// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";

import { applyUiFont } from "../../src/utils/uiFont";

describe("UI font preference", () => {
  it("applies a saved font verbatim and clears a blank preference", () => {
    applyUiFont("Iosevka, monospace");
    expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe(
      "Iosevka, monospace",
    );

    applyUiFont(
      'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
    );
    expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe(
      'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif',
    );
    applyUiFont("");
    expect(document.documentElement.style.getPropertyValue("--font-ui")).toBe("");
  });
});

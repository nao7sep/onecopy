import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Style guard (app-chrome conventions): the scrollbar rules that actually
// change pixels must stay present in App.css. The load-bearing one is
// scrollbar-color — a Chromium webview that sees scrollbar-width alone
// IGNORES every ::-webkit-scrollbar rule and shows the UA default bar.

const css = readFileSync(join(__dirname, "../../src/App.css"), "utf8");

describe("scrollbar styling", () => {
  it("sets scrollbar-color alongside scrollbar-width", () => {
    expect(css).toMatch(/scrollbar-width:\s*thin/);
    expect(css).toMatch(/scrollbar-color:\s*var\(--scrollbar-thumb\)\s+transparent/);
  });

  it("insets the thumb as a pill and brightens it on hover", () => {
    expect(css).toMatch(/::-webkit-scrollbar-thumb\s*{[^}]*background-clip:\s*padding-box/);
    expect(css).toMatch(/::-webkit-scrollbar-thumb:hover/);
  });

  it("keeps the track and corner transparent", () => {
    expect(css).toMatch(/::-webkit-scrollbar-track\s*{[^}]*transparent/);
    expect(css).toMatch(/::-webkit-scrollbar-corner\s*{[^}]*transparent/);
  });

  it("themes the thumb in both modes", () => {
    const thumbDefs = css.match(/--scrollbar-thumb:/g) ?? [];
    expect(thumbDefs.length).toBeGreaterThanOrEqual(2);
  });
});

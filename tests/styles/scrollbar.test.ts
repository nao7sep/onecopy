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

  it("themes the thumb differently in each mode", () => {
    // Counting occurrences proved nothing: two definitions in the same block
    // with the same value passed. What matters is that the dark-mode value
    // lives in a DIFFERENT block and actually differs.
    // Anchored to the start of a line: ".dark" also occurs in prose comments,
    // and a bare indexOf found one of those and then walked forward into the
    // :root block, comparing it against itself.
    const blockAfter = (selector: string): string => {
      const at = css.search(new RegExp(`^${selector.replace(".", "\\.")}\\s*\\{`, "m"));
      expect(at, `${selector} must open a block in App.css`).toBeGreaterThanOrEqual(0);
      const open = css.indexOf("{", at);
      const close = css.indexOf("\n}", open);
      return css.slice(open, close);
    };
    const valueIn = (block: string): string => {
      const match = block.match(/--scrollbar-thumb:\s*([^;]+);/);
      expect(match, "the block must define --scrollbar-thumb").toBeTruthy();
      return match![1]!.trim();
    };
    expect(valueIn(blockAfter(":root"))).not.toBe(valueIn(blockAfter(".dark")));
  });
});

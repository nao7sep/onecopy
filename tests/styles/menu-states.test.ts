import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

const css = readFileSync("src/App.css", "utf8");

it("keeps menu hover and press separate from its inset keyboard mark", () => {
  // Pin the chosen treatments, not a claim about rendered contrast. The
  // keyboard selector includes both menu attributes and follows hover, so
  // hovering the focused command cannot replace its keyboard indication.
  const hover = css.match(/\.oc-menu-item:enabled:hover \{([^}]+)\}/);
  const press = css.match(/\.oc-menu-item:enabled:active \{([^}]+)\}/);
  const keyboard = css.match(/\[role="menu"\]\[data-keyboard-navigation="true"\] \.oc-menu-item:focus \{([^}]+)\}/);
  expect(hover?.[1]).toContain("background-color: var(--surface-muted)");
  expect(press?.[1]).toContain("background-color: var(--surface-pressed)");
  expect(press!.index).toBeGreaterThan(hover!.index!);
  expect(keyboard?.[1]).not.toContain("background-color");
  expect(keyboard?.[1]).toContain("outline: 2px solid var(--focus-ring)");
  expect(keyboard?.[1]).toContain("outline-offset: -2px");
  expect(keyboard!.index).toBeGreaterThan(hover!.index!);
});

import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// A utility that names a colour the theme never defines emits no rule at all, so
// the control simply inherits whatever is around it and nothing reports the
// mistake: the activity trace's path link asked for `text-link`, there is no
// `--color-link`, and the link rendered in body ink. Nothing catches that shape —
// the palette test reads tokens, not the utilities that spend them.
const ROOT = fileURLToPath(new URL("../../src", import.meta.url));
const css = readFileSync(join(ROOT, "App.css"), "utf8");
const TOKENS = new Set([...css.matchAll(/--color-([a-z0-9-]+)\s*:/g)].map(([, name]) => name));

// Tailwind's own palette, which the app uses directly for a few status washes.
const PALETTE =
  /^(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3}$/;
// Utilities that share a prefix with the colour ones but carry no colour.
const NOT_A_COLOUR =
  /^(?:xs|sm|base|lg|xl|\dxl|left|right|center|justify|start|end|wrap|nowrap|balance|pretty|ellipsis|clip|none|solid|dashed|dotted|hidden|current|transparent|inherit|black|white|inset|b-0|\d+|[a-z])$/;
const COLOUR_UTILITY =
  /^(?:[a-z-]+:)*(?:text|bg|border|outline|ring|decoration|fill|stroke)-([a-z][a-z0-9-]*?)(?:\/\d+)?$/;

function tsxSources(): Array<readonly [string, string]> {
  const walk = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
      const path = join(dir, entry.name);
      return entry.isDirectory() ? walk(path) : entry.name.endsWith(".tsx") ? [path] : [];
    });
  return walk(ROOT).map((path) => [relative(ROOT, path), readFileSync(path, "utf8")] as const);
}

describe("colour utilities", () => {
  it("spends only colours the theme defines", () => {
    const undefinedUses: string[] = [];
    for (const [name, source] of tsxSources())
      for (const [, classes] of source.matchAll(/className=(?:\{)?[`"]([^`"]*)[`"]/g))
        for (const utility of classes.split(/\s+/)) {
          const colour = utility.match(COLOUR_UTILITY)?.[1];
          if (colour === undefined) continue;
          if (TOKENS.has(colour) || PALETTE.test(colour) || NOT_A_COLOUR.test(colour)) continue;
          undefinedUses.push(`${name}: ${utility}`);
        }
    expect(undefinedUses).toEqual([]);
  });
});

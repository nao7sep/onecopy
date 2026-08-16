import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

// The product name in user-facing text.
//
// A rollout once shipped ~20 startup dialogs saying "onecopy could not start" — the
// lowercase REPO name, not the product name — and neither the suites nor a 15-agent
// review caught one, because nothing checked that the app spells its own name right.
// The text a user reads at the worst moment is exactly where it matters most.
//
// The rule: a user-facing string never contains the bare lowercase repo name. Code
// identifiers, import paths, URL schemes, log tags and CSS class lists legitimately
// do, so only literals that READ AS PROSE are checked.
const PRODUCT = "OneCopy";
const REPO = "onecopy";

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules" || entry.startsWith(".")) continue;
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) out.push(...sourceFiles(full));
    else if (/\.(ts|tsx)$/.test(entry)) out.push(full);
  }
  return out;
}

/** Prose = several plain words. A CSS class list, a path, or a scheme is not. */
function readsAsProse(literal: string): boolean {
  const tokens = literal.trim().split(/\s+/);
  if (tokens.length < 3) return false;
  // Any styling/pathing punctuation in a token marks the literal as machine text.
  if (tokens.some((t) => /[:[\]/\\{}=]|--|^-|-$/.test(t))) return false;
  // Tailwind-ish: most tokens hyphenated.
  const hyphenated = tokens.filter((t) => t.includes("-")).length;
  return hyphenated / tokens.length < 0.3;
}

describe("product name in user-facing text", () => {
  it(`spells the product "${PRODUCT}", never the bare repo name`, () => {
    const offenders: string[] = [];
    const bareRepo = new RegExp(`\\b${REPO}\\b`);
    for (const file of sourceFiles("src")) {
      const text = readFileSync(file, "utf8");
      for (const [lineNo, line] of text.split("\n").entries()) {
        const trimmed = line.trim();
        if (trimmed.startsWith("//") || trimmed.startsWith("*") || trimmed.startsWith("import")) continue;
        if (line.includes("className") || line.includes("class=")) continue;
        for (const m of line.matchAll(/["'`]([^"'`]{12,})["'`]/g)) {
          const literal = m[1] ?? "";
          if (!readsAsProse(literal)) continue;
          if (bareRepo.test(literal) && !literal.includes(PRODUCT)) {
            offenders.push(`${file}:${lineNo + 1}: ${literal.slice(0, 80)}`);
          }
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Cut text shows one marker, and CSS draws it: a line that stops text from
// wrapping and hides what overflows must also end it with an ellipsis, or the
// reader cannot tell the text was cut. A clip with no marker fails.

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(tsx?|css)$/.test(entry.name) ? [path] : [];
  });
}

/** The places in one file that clip text without a marker, as path:unit. */
function unmarkedClips(path: string, text: string): string[] {
  const units = path.endsWith(".css") ? text.split("}") : text.split("\n");
  return units.flatMap((unit, index) => {
    const nowrap = /whitespace-nowrap|white-space:\s*nowrap/.test(unit);
    const hidden = /overflow(-x)?-(hidden|clip)\b|overflow(-x)?:\s*(hidden|clip)/.test(unit);
    const marked = /\btruncate\b|\btext-ellipsis\b|text-overflow:\s*ellipsis/.test(unit);
    const clipped = /\btext-clip\b|text-overflow:\s*clip/.test(unit) || (nowrap && hidden && !marked);
    return clipped ? [`${path}:${index + 1}`] : [];
  });
}

describe("text truncation", () => {
  it("never clips text without a marker", () => {
    const clips = sourceFiles("src").flatMap((path) => unmarkedClips(path, readFileSync(path, "utf8")));
    expect(clips).toEqual([]);
  });

  it.each([
    ["planted.tsx", '<span className="overflow-hidden whitespace-nowrap">'],
    ["planted.tsx", '<span className="text-clip">'],
    ["planted.css", ".name { white-space: nowrap; overflow: hidden; }"],
  ])("finds a planted unmarked clip in %s: %s", (path, text) => {
    expect(unmarkedClips(path, text)).toEqual([`${path}:1`]);
  });

  it("accepts a clip that ends with an ellipsis", () => {
    expect(unmarkedClips("planted.tsx", '<span className="truncate whitespace-nowrap overflow-hidden">')).toEqual([]);
  });
});

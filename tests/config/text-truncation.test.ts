import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Cut text shows one marker (localization-conventions, "Ellipsis"): CSS draws
// it, so a line that stops text from wrapping and hides what overflows must
// also end it with an ellipsis. A clip with no marker fails.

// The working tree, not the index: a new file is checked before it is
// committed, not after.
function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(tsx?|css)$/.test(entry.name) ? [path] : [];
  });
}

describe("text truncation", () => {
  it("never clips text without a marker", () => {
    const clips: string[] = [];
    for (const path of sourceFiles("src")) {
      const text = readFileSync(path, "utf8");
      const units = path.endsWith(".css") ? text.split("}") : text.split("\n");
      units.forEach((unit, index) => {
        const nowrap = /whitespace-nowrap|white-space:\s*nowrap/.test(unit);
        const hidden = /overflow(-x)?-(hidden|clip)\b|overflow(-x)?:\s*(hidden|clip)/.test(unit);
        const marked = /\btruncate\b|\btext-ellipsis\b|text-overflow:\s*ellipsis/.test(unit);
        if (/\btext-clip\b|text-overflow:\s*clip/.test(unit) || (nowrap && hidden && !marked)) {
          clips.push(`${path}:${index + 1}`);
        }
      });
    }
    expect(clips).toEqual([]);
  });
});

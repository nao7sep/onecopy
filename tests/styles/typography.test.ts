import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// 11px is the smallest text the interface draws, for tertiary labels such as
// snapshot timestamps; anything smaller stops being readable at ordinary
// viewing distance.
const FLOOR_PX = 11;

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.tsx?$/.test(entry.name) ? [path] : [];
  });
}

/** Arbitrary pixel text sizes below the floor, as path: class. */
function belowFloor(path: string, text: string): string[] {
  return [...text.matchAll(/\btext-\[(\d+(?:\.\d+)?)px\]/g)]
    .filter((match) => Number(match[1]) < FLOOR_PX)
    .map((match) => `${path}: ${match[0]}`);
}

describe("text size floor", () => {
  it("draws no text smaller than the floor", () => {
    const found = sourceFiles("src").flatMap((path) => belowFloor(path, readFileSync(path, "utf8")));
    expect(found).toEqual([]);
  });

  it("finds a planted size below the floor and accepts the floor itself", () => {
    expect(belowFloor("planted.tsx", '<span className="text-[10px]">')).toEqual(["planted.tsx: text-[10px]"]);
    expect(belowFloor("planted.tsx", '<span className="text-[11px]">')).toEqual([]);
  });
});

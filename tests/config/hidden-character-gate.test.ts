import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const script = path.resolve("scripts/check-hidden-characters.mjs");
const temporaryRoots: string[] = [];

afterEach(() => {
  for (const root of temporaryRoots.splice(0)) rmSync(root, { recursive: true });
});

function fixture(contents: string): string {
  const root = mkdtempSync(path.join(tmpdir(), "onecopy-hidden-characters-"));
  temporaryRoots.push(root);
  writeFileSync(path.join(root, "source.ts"), contents);
  return root;
}

function run(root: string) {
  return spawnSync(process.execPath, [script, root], { encoding: "utf8" });
}

describe("hidden-character gate", () => {
  it("accepts ordinary source text", () => {
    const result = run(fixture("export const value = 'visible';\n"));
    expect(result.status).toBe(0);
    expect(result.stdout).toContain("No hidden characters found");
  });

  it("rejects planted control and invisible Unicode characters", () => {
    const root = fixture(`left${String.fromCodePoint(1)}middle${String.fromCodePoint(0x200b)}right`);
    const result = run(root);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("U+0001");
    expect(result.stderr).toContain("U+200B");
  });
});

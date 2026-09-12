import { readFileSync, readdirSync } from "node:fs";
import { basename, join } from "node:path";
import { describe, expect, it } from "vitest";
import { cargoSuiteTargets } from "./support/cargo-manifest";

const crateRoot = "src-tauri";
const testsRoot = join(crateRoot, "tests");
const manifest = readFileSync(join(crateRoot, "Cargo.toml"), "utf8");

function captures(source: string, pattern: RegExp): string[] {
  return [...source.matchAll(pattern)].map((match) => match[1]!);
}

describe("the Rust integration harness inventory", () => {
  it("assigns every root test module to exactly one explicit target", () => {
    const rootModules = readdirSync(testsRoot)
      .filter((name) => name.endsWith(".rs"))
      .sort();
    const suitePaths = captures(manifest, /^path = "(tests\/suites\/[^\"]+\.rs)"$/gm);
    expect(suitePaths.length).toBeGreaterThan(0);

    const included = suitePaths.flatMap((path) =>
      captures(
        readFileSync(join(crateRoot, path), "utf8"),
        /^#\[path = "\.\.\/([^\"]+\.rs)"\]$/gm,
      ),
    );
    const standalone = captures(
      manifest,
      /^path = "tests\/([^/\"]+\.rs)"$/gm,
    );
    const assigned = [...included, ...standalone].sort();

    expect(new Set(assigned).size, "a test module is assigned more than once").toBe(
      assigned.length,
    );
    expect(assigned).toEqual(rootModules);
  });

  it("keeps suite target names aligned with their files", () => {
    const targets = cargoSuiteTargets(manifest);
    expect(targets.length).toBeGreaterThan(0);
    for (const { name, path } of targets) {
      expect(name).toBe(`${basename(path, ".rs")}_test_suite`);
    }
  });
});

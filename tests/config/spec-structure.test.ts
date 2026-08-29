import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const script = path.resolve("scripts/check-spec-structure.mjs");
const temporaryRoots: string[] = [];

afterEach(() => {
  for (const root of temporaryRoots.splice(0)) rmSync(root, { recursive: true });
});

function fixture(index: string, files: Record<string, string>): string {
  const root = mkdtempSync(path.join(tmpdir(), "onecopy-spec-structure-"));
  temporaryRoots.push(root);
  const specs = path.join(root, "specs");
  mkdirSync(specs, { recursive: true });
  writeFileSync(path.join(specs, "index.md"), index);
  for (const [name, contents] of Object.entries(files)) {
    const target = path.join(specs, name);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, contents);
  }
  return root;
}

function run(root: string) {
  return spawnSync(process.execPath, [script, root], { encoding: "utf8" });
}

const validIndex = `# Spec Index

| File | Solely owns | Explicitly excludes |
|---|---|---|
| \`behavior.md\` | Behavior | Mechanics |
`;

describe("spec structure gate", () => {
  it("accepts one routed, nonempty contract", () => {
    const result = run(fixture(validIndex, { "behavior.md": "# Behavior\n" }));
    expect(result.status).toBe(0);
    expect(result.stdout).toContain("Spec structure is valid");
  });

  it("rejects planted empty, unlisted, duplicate, missing, and catch-all contracts", () => {
    const root = fixture(
      `# Spec Index

| File | Solely owns | Explicitly excludes |
|---|---|---|
| \`behavior.md\` | Behavior | Mechanics |
| \`behavior.md\` | Duplicate | Mechanics |
| \`missing.md\` | Missing | Mechanics |
`,
      {
        "behavior.md": "",
        "extra.md": "# Extra\n",
        "general.md": "# General\n",
      },
    );
    const result = run(root);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("specs/behavior.md is empty");
    expect(result.stderr).toContain("specs/behavior.md is routed 2 times");
    expect(result.stderr).toContain("specs/extra.md is not routed");
    expect(result.stderr).toContain("specs/general.md uses an invalid catch-all filename");
    expect(result.stderr).toContain("specs/index.md routes missing specs/missing.md");
  });

  it("rejects a planted broken local Markdown reference", () => {
    const root = fixture(validIndex, {
      "behavior.md": "# Behavior\n\n[Missing details](missing-details.md)\n",
    });
    const result = run(root);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("broken reference");
  });

  it("rejects a routing header without its table separator", () => {
    const root = fixture(validIndex.replace("|---|---|---|", "plain text"), {
      "behavior.md": "# Behavior\n",
    });
    const result = run(root);
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("exactly one routing table");
  });
});

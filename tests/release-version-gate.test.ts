import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

import { parseJsonVersion } from "./helpers/versions";

const script = path.resolve("scripts/check-release-version.mjs");

function readTauriVersion(): string {
  return parseJsonVersion(readFileSync("src-tauri/tauri.conf.json", "utf8"));
}

function run(refType: string, refName: string): string {
  return execFileSync(process.execPath, [script], {
    encoding: "utf8",
    env: { ...process.env, GITHUB_REF_TYPE: refType, GITHUB_REF_NAME: refName },
  });
}

describe("release version gate", () => {
  it("accepts the v-prefixed Tauri source-of-truth version", () => {
    expect(run("tag", `v${readTauriVersion()}`)).toContain("matches the Tauri version");
  });

  it("rejects a tag that disagrees with the packaged version", () => {
    expect(() => run("tag", "v999.0.0")).toThrow(/must equal/);
  });

  it("does not require a tag for manual build dispatches", () => {
    expect(run("branch", "scratch")).toContain("gate skipped");
  });
});

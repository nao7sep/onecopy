import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { sourceState } from "./source-state.mjs";

function git(root, args) {
  execFileSync("git", ["-C", root, ...args], { stdio: "ignore" });
}

test("untracked symlinks hash their link text without reading the target", {
  skip: process.platform === "win32" && "ordinary Windows CI cannot assume symlink privilege",
}, () => {
  const parent = mkdtempSync(join(tmpdir(), "onecopy-source-state-"));
  const repository = join(parent, "repository");
  const outside = join(parent, "outside.txt");
  try {
    git(parent, ["init", "repository"]);
    writeFileSync(join(repository, "tracked.txt"), "tracked\n");
    git(repository, ["add", "tracked.txt"]);
    git(repository, ["-c", "user.name=OneCopy Test", "-c", "user.email=test@example.invalid", "commit", "-m", "Initial"]);
    writeFileSync(outside, "first private value\n");
    symlinkSync(outside, join(repository, "external-link"));

    const before = sourceState(repository);
    writeFileSync(outside, "different private value\n");
    const after = sourceState(repository);

    assert.equal(before.untrackedCount, 1);
    assert.equal(after.untrackedCount, 1);
    assert.equal(before.untrackedContentSha256, after.untrackedContentSha256);
  } finally {
    rmSync(parent, { recursive: true, force: true });
  }
});

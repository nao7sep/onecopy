import assert from "node:assert/strict";
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { cloneArtifactTree } from "./artifact-tree.mjs";
import { publishVersionedBinary, runPreparationStep } from "./prepare.mjs";

const roots = [];
test.afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function temporaryRoot(label) {
  const root = join(tmpdir(), `onecopy-ai-prepare-${process.pid}-${label}-${Date.now()}`);
  mkdirSync(root, { recursive: true });
  roots.push(root);
  return root;
}

test("preparation subprocesses are bounded and cancellable", async () => {
  await assert.rejects(
    runPreparationStep(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
      cwd: process.cwd(),
      timeout: 200,
      capture: true,
    }),
    /timed out/,
  );

  const controller = new AbortController();
  const interrupted = runPreparationStep(
    process.execPath,
    ["-e", "setInterval(() => {}, 1000)"],
    { cwd: process.cwd(), timeout: 10_000, capture: true, signal: controller.signal },
  );
  setTimeout(() => controller.abort(), 100);
  await assert.rejects(interrupted, /interrupted/);
});

test("digest publication preserves prior binaries and removes private staging", () => {
  const root = temporaryRoot("publication");
  const source = join(root, "source.bin");
  const output = join(root, "output");
  writeFileSync(source, "first");
  const first = publishVersionedBinary(source, output, "onecopy");
  writeFileSync(source, "second");
  const second = publishVersionedBinary(source, output, "onecopy");

  assert.notEqual(first.basename, second.basename);
  assert.equal(readFileSync(join(output, first.basename), "utf8"), "first");
  assert.equal(readFileSync(join(output, second.basename), "utf8"), "second");
  assert.equal(readdirSync(output).some((name) => name.endsWith(".partial")), false);
});

test("prepared artifact reuse is idempotent and preserves existing bytes", () => {
  const root = temporaryRoot("reuse");
  const source = join(root, "source");
  const output = join(root, "output");
  mkdirSync(source);
  writeFileSync(join(source, "model.bin"), "verified model");
  cloneArtifactTree(source, output);
  cloneArtifactTree(source, output);
  assert.equal(readFileSync(join(output, "model.bin"), "utf8"), "verified model");
});

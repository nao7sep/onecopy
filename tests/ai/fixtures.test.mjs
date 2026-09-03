import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { indexFixtureRoot, materializeFixtures, resolveFixtures } from "./fixtures.mjs";

const roots = [];
test.afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function fixtureRoot(label) {
  const root = join(tmpdir(), `onecopy-ai-fixtures-${process.pid}-${label}-${Date.now()}`);
  mkdirSync(root, { recursive: true });
  roots.push(root);
  return root;
}

function reference(name, bytes) {
  return {
    basename: name,
    bytes: bytes.length,
    sha256: createHash("sha256").update(bytes).digest("hex"),
  };
}

test("resolver scans once and materializes byte-identical fixtures", () => {
  const root = fixtureRoot("copy");
  const nested = join(root, "nested");
  mkdirSync(nested);
  const bytes = Buffer.from("canonical fixture");
  writeFileSync(join(nested, "sample.bin"), bytes);
  const matches = resolveFixtures(indexFixtureRoot(root), [reference("sample.bin", bytes)]);
  const outputRoot = fixtureRoot("output");
  const outputs = materializeFixtures(outputRoot, matches);
  assert.deepEqual(readFileSync(outputs[0]), bytes);
});

test("missing, wrong-hash, duplicate, case-variant, and symlink inputs fail", () => {
  const root = fixtureRoot("failures");
  const bytes = Buffer.from("same");
  const ref = reference("sample.bin", bytes);
  assert.throws(() => resolveFixtures(indexFixtureRoot(root), [ref]), /found 0/);
  writeFileSync(join(root, ref.basename), Buffer.from("nope"));
  assert.throws(() => resolveFixtures(indexFixtureRoot(root), [ref]), /found 0/);
  writeFileSync(join(root, ref.basename), bytes);
  mkdirSync(join(root, "duplicate"));
  writeFileSync(join(root, "duplicate", ref.basename), bytes);
  assert.throws(() => resolveFixtures(indexFixtureRoot(root), [ref]), /found 2/);
  rmSync(join(root, "duplicate"), { recursive: true });
  mkdirSync(join(root, "case-variant"));
  writeFileSync(join(root, "case-variant", "SAMPLE.BIN"), bytes);
  assert.throws(() => resolveFixtures(indexFixtureRoot(root), [ref]), /case-variant/);
  const linkRoot = fixtureRoot("link");
  try {
    symlinkSync(join(root, ref.basename), join(linkRoot, ref.basename));
    assert.throws(() => resolveFixtures(indexFixtureRoot(linkRoot), [ref]), /found 0/);
  } catch (error) {
    if (error?.code !== "EPERM") throw error;
  }
});

test("post-resolution replacement is detected", () => {
  const root = fixtureRoot("replace");
  const bytes = Buffer.from("before");
  const path = join(root, "sample.bin");
  writeFileSync(path, bytes);
  const matches = resolveFixtures(indexFixtureRoot(root), [reference("sample.bin", bytes)]);
  writeFileSync(path, Buffer.from("after!"));
  assert.throws(() => materializeFixtures(fixtureRoot("replace-output"), matches), /changed after preflight/);
});

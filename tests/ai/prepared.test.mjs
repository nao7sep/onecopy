import assert from "node:assert/strict";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { assertPreparedUnchanged, snapshotPreparedGuard } from "./prepared.mjs";

function fixture(label) {
  const root = mkdtempSync(join(tmpdir(), `onecopy-prepared-guard-${label}-`));
  const managedRoot = join(root, "managed");
  const modelRoot = join(managedRoot, "models");
  const scenarioExecutable = join(root, "onecopy-ai-scenario-test");
  mkdirSync(modelRoot, { recursive: true });
  writeFileSync(scenarioExecutable, "scenario");
  return { root, managedRoot, modelRoot, scenarioExecutable };
}

test("prepared guard uses file metadata without reading artifact bytes", {
  skip: process.platform === "win32" && "Windows chmod does not make an owner file unreadable",
}, () => {
  const paths = fixture("metadata");
  const artifact = join(paths.modelRoot, "model.bin");
  try {
    writeFileSync(artifact, "bytes that the guard must not read");
    chmodSync(artifact, 0o000);
    const guard = snapshotPreparedGuard(paths.managedRoot, paths.scenarioExecutable, [{
      id: "model",
      relativePath: join("models", "model.bin"),
    }]);

    assert.doesNotThrow(() => assertPreparedUnchanged(guard));
  } finally {
    rmSync(paths.root, { recursive: true, force: true });
  }
});

test("prepared guard rejects registry paths outside the managed root", () => {
  const paths = fixture("escape");
  try {
    assert.throws(() => snapshotPreparedGuard(
      paths.managedRoot,
      paths.scenarioExecutable,
      [{ id: "model", relativePath: join("..", "outside.bin") }],
    ), /escapes its prepared root/);
  } finally {
    rmSync(paths.root, { recursive: true, force: true });
  }
});

test("prepared guard rejects symlinks instead of following their targets", {
  skip: process.platform === "win32" && "ordinary Windows CI cannot assume symlink privilege",
}, () => {
  const paths = fixture("symlink");
  const outside = join(paths.root, "outside");
  try {
    rmSync(paths.modelRoot, { recursive: true });
    mkdirSync(outside);
    writeFileSync(join(outside, "model.bin"), "private bytes");
    symlinkSync(outside, paths.modelRoot);
    assert.throws(() => snapshotPreparedGuard(
      paths.managedRoot,
      paths.scenarioExecutable,
      [{ id: "model", relativePath: join("models", "model.bin") }],
    ), /symbolic links/);
  } finally {
    rmSync(paths.root, { recursive: true, force: true });
  }
});

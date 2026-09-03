import assert from "node:assert/strict";
import { mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { assertPrivacySafe, compatibleResults, safeFailure, writeAtomicReport } from "./report.mjs";

const result = {
  schemaVersion: 1,
  profileId: "standard",
  profileVersion: 1,
  outcome: "passed",
  machine: { platform: "win32", osVersion: "10.0", architecture: "x64", cpuModel: "CPU", logicalCpuCount: 2, totalMemoryBytes: 1 },
  cases: [{ id: "face", dependencies: [{ id: "model", sha256: "a".repeat(64) }], fixtures: [{ basename: "face.jpg", sha256: "b".repeat(64) }] }],
};

test("privacy gate accepts portable facts and rejects private fields and paths", () => {
  assert.equal(assertPrivacySafe(result), result);
  for (const candidate of [
    { hostname: "private" },
    { username: "private" },
    { transcript: "hello" },
    { nested: { commandLine: "cargo test" } },
    { failure: "C:\\Users\\Someone\\secret.jpg" },
    { failure: "/Users/someone/secret.jpg" },
  ]) {
    assert.throws(() => assertPrivacySafe(candidate), /prohibited|path-shaped/);
  }
  assert.deepEqual(safeFailure("fixture", new Error("C:\\Users\\Me\\secret.jpg")), {
    category: "fixture",
    message: "<local-path>",
  });
});

test("comparison rejects incompatible fixture identity", () => {
  assert.equal(compatibleResults(result, structuredClone(result)), true);
  const changed = structuredClone(result);
  changed.cases[0].fixtures[0].sha256 = "c".repeat(64);
  assert.throws(() => compatibleResults(result, changed), /different cases/);
});

test("partial reports are atomically replaceable", () => {
  const root = join(tmpdir(), `onecopy-ai-report-${process.pid}-${Date.now()}`);
  mkdirSync(root);
  const path = join(root, "result.json");
  try {
    writeAtomicReport(path, { ...result, outcome: "running" });
    writeAtomicReport(path, result);
    assert.equal(JSON.parse(readFileSync(path, "utf8")).outcome, "passed");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

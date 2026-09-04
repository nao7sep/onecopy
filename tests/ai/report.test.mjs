import assert from "node:assert/strict";
import { mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { compare } from "./compare.mjs";
import { validateResult } from "./contracts.mjs";
import {
  assertPrivacySafe,
  compatibleResults,
  recoverInterruptedReport,
  requireUnusedReportPath,
  safeConsoleMessage,
  writeAtomicReport,
} from "./report.mjs";

const result = {
  schemaVersion: 2,
  profileId: "standard",
  profileVersion: 1,
  mode: "benchmark",
  outcome: "passed",
  startedAtUtc: "2026-09-04T00:00:00.000Z",
  finishedAtUtc: "2026-09-04T00:00:01.000Z",
  source: {
    commit: "a".repeat(40),
    dirty: false,
    trackedDiffSha256: "e".repeat(64),
    untrackedCount: 0,
    untrackedContentSha256: "f".repeat(64),
  },
  executable: { basename: "onecopy-ai-scenario", sha256: "c".repeat(64) },
  buildManifestSha256: "d".repeat(64),
  build: {
    platform: "win32",
    architecture: "x64",
    targetTriple: "x86_64-pc-windows-msvc",
    toolchain: { rustc: "rustc test", cargo: "cargo test", node: "v26.0.0" },
    compileFeatures: ["ai-test-support"],
    capabilities: [
      { feature: "face-scoring", options: ["none"] },
      { feature: "transcription", options: ["none"] },
    ],
  },
  machine: { platform: "win32", osVersion: "10.0", architecture: "x64", cpuModel: "CPU", logicalCpuCount: 2, totalMemoryBytes: 1 },
  cases: [{
    scenarioId: "face",
    timeoutMs: 10_000,
    outcome: "passed",
    startedAtUtc: "2026-09-04T00:00:00.000Z",
    finishedAtUtc: "2026-09-04T00:00:01.000Z",
    dependencies: [{ id: "model", sha256: "a".repeat(64), bytes: 1, version: null }],
    fixtures: [{ basename: "face.jpg", sha256: "b".repeat(64), bytes: 1 }],
    configuredAcceleration: "none",
    observedAcceleration: null,
    correctness: { ready: 1 },
    observations: { wallMs: 10, peakProcessTreeBytes: 100, phases: [] },
  }],
};

test("privacy gate accepts portable facts and rejects private fields and paths", () => {
  assert.equal(assertPrivacySafe(result), result);
  for (const candidate of [
    { hostname: "private" },
    { username: "private" },
    { transcript: "hello" },
    { driveIdentity: "private" },
    { nested: { commandLine: "cargo test" } },
    { failure: "C:\\Users\\Someone\\secret.jpg" },
    { failure: "/Users/someone/secret.jpg" },
    { failure: "/etc/passwd" },
  ]) {
    assert.throws(() => assertPrivacySafe(candidate), /prohibited|path-shaped/);
  }
  assert.equal(assertPrivacySafe({ driver: { sha256: "a".repeat(64) } }).driver.sha256.length, 64);
  for (const message of [
    "C:\\Users\\Me\\secret.jpg",
    "/Users/me/secret.jpg",
    "/etc/passwd",
    "ENOENT: no such file, open '/Volumes/private/file'",
  ]) {
    assert.equal(
      safeConsoleMessage(new Error(message)),
      "The command could not finish because a local path was unavailable.",
    );
  }
  assert.equal(safeConsoleMessage(new Error("first\nsecond")), "first second");
  assert.equal(safeConsoleMessage(new Error("see https://example.com/help")), "see https://example.com/help");
});

test("comparison rejects incompatible fixture identity", () => {
  assert.deepEqual(compatibleResults(result, structuredClone(result)), { machineFactsMatch: true });
  const changed = structuredClone(result);
  changed.cases[0].fixtures[0].sha256 = "c".repeat(64);
  assert.throws(() => compatibleResults(result, changed), /different cases/);
  const differentTimeout = structuredClone(result);
  differentTimeout.cases[0].timeoutMs += 1;
  assert.throws(() => compatibleResults(result, differentTimeout), /different cases/);
  const differentBuild = structuredClone(result);
  differentBuild.build.toolchain.node = "v27.0.0";
  assert.throws(() => compatibleResults(result, differentBuild), /different build facts/);
});

test("result validation rejects incomplete evidence and accepts unavailable failed observations", () => {
  const incomplete = structuredClone(result);
  delete incomplete.source.trackedDiffSha256;
  assert.throws(() => validateResult(incomplete), /trackedDiffSha256/);

  const failed = structuredClone(result);
  failed.outcome = "failed";
  failed.cases[0].outcome = "failed";
  failed.cases[0].failure = { category: "scenario-runner", message: "scenario did not launch" };
  failed.cases[0].observations = null;
  delete failed.cases[0].correctness;
  assert.equal(validateResult(failed), failed);

  const running = structuredClone(result);
  running.outcome = "running";
  delete running.finishedAtUtc;
  running.cases[0].outcome = "running";
  delete running.cases[0].finishedAtUtc;
  delete running.cases[0].correctness;
  assert.throws(() => validateResult(running), /running observations must be null/);

  const mismatchedAcceleration = structuredClone(result);
  mismatchedAcceleration.cases[0].scenarioId = "audio-transcription";
  mismatchedAcceleration.cases[0].configuredAcceleration = "none";
  mismatchedAcceleration.cases[0].observedAcceleration = "metal";
  assert.throws(() => validateResult(mismatchedAcceleration), /differs from configuredAcceleration/);
});

test("partial reports are atomically replaceable", () => {
  const root = join(tmpdir(), `onecopy-ai-report-${process.pid}-${Date.now()}`);
  mkdirSync(root);
  const path = join(root, "result.json");
  try {
    const partial = structuredClone(result);
    partial.outcome = "running";
    delete partial.finishedAtUtc;
    partial.cases = [];
    writeAtomicReport(path, partial);
    writeAtomicReport(path, result);
    assert.equal(JSON.parse(readFileSync(path, "utf8")).outcome, "passed");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a new run never overwrites an existing terminal report", () => {
  const root = join(tmpdir(), `onecopy-ai-collision-${process.pid}-${Date.now()}`);
  mkdirSync(root);
  const path = join(root, "result.json");
  try {
    writeAtomicReport(path, result);
    assert.throws(() => requireUnusedReportPath(path), /already exists/);
    assert.deepEqual(JSON.parse(readFileSync(path, "utf8")), result);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a stale running report is recoverable without erasing completed cases", () => {
  const root = join(tmpdir(), `onecopy-ai-recovery-${process.pid}-${Date.now()}`);
  mkdirSync(root);
  const path = join(root, "result.json");
  try {
    const partial = structuredClone(result);
    partial.outcome = "running";
    delete partial.finishedAtUtc;
    partial.cases.push({
      ...partial.cases[0],
      scenarioId: "audio-transcription",
      outcome: "running",
      configuredAcceleration: "none",
      observedAcceleration: null,
      observations: null,
    });
    delete partial.cases[1].finishedAtUtc;
    delete partial.cases[1].correctness;
    writeAtomicReport(path, partial);
    assert.equal(recoverInterruptedReport(path), true);
    const recovered = JSON.parse(readFileSync(path, "utf8"));
    assert.equal(recovered.outcome, "interrupted");
    assert.equal(recovered.cases.length, 2);
    assert.equal(recovered.cases[0].outcome, "passed");
    assert.equal(recovered.cases[1].outcome, "interrupted");
    assert.equal(recoverInterruptedReport(path), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("recovery preserves a completed case when the runner stops between cases", () => {
  const root = join(tmpdir(), `onecopy-ai-between-cases-${process.pid}-${Date.now()}`);
  mkdirSync(root);
  const path = join(root, "result.json");
  try {
    const partial = structuredClone(result);
    partial.outcome = "running";
    delete partial.finishedAtUtc;
    writeAtomicReport(path, partial);
    assert.equal(recoverInterruptedReport(path), true);
    const recovered = validateResult(JSON.parse(readFileSync(path, "utf8")));
    assert.equal(recovered.outcome, "interrupted");
    assert.equal(recovered.cases[0].outcome, "passed");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("comparison requires identity and reports observation availability truthfully", () => {
  const root = join(tmpdir(), `onecopy-ai-compare-${process.pid}-${Date.now()}`);
  mkdirSync(root);
  const leftPath = join(root, "left.json");
  const rightPath = join(root, "right.json");
  const measured = {
    ...result,
    cases: [{
      ...result.cases[0],
      observations: {
        wallMs: 10,
        peakProcessTreeBytes: null,
        phases: [
          { phase: "inference", wallMs: 2 },
          { phase: "inference", wallMs: 3 },
          { phase: "input-decode", wallMs: 1 },
        ],
      },
    }],
  };
  try {
    writeAtomicReport(leftPath, measured);
    writeAtomicReport(rightPath, measured);
    const comparison = compare(leftPath, rightPath).cases[0];
    assert.equal(comparison.correctnessEquivalent, true);
    assert.equal(comparison.phaseTimeRatios.inference, 1);
    assert.equal(comparison.peakMemoryDifferenceBytes, null);

    const otherMachine = structuredClone(measured);
    otherMachine.machine.cpuModel = "Different CPU";
    otherMachine.cases[0].observations.phases.pop();
    writeAtomicReport(rightPath, otherMachine);
    const descriptive = compare(leftPath, rightPath);
    assert.equal(descriptive.machineFactsMatch, false);
    assert.equal(descriptive.crossMachineUse, "descriptive-only");
    assert.equal(descriptive.cases[0].wallTimeRatio, null);
    assert.equal(descriptive.cases[0].phaseTimeRatios["input-decode"], null);

    const running = structuredClone(measured);
    running.outcome = "running";
    delete running.finishedAtUtc;
    running.cases = [];
    writeAtomicReport(rightPath, running);
    assert.throws(() => compare(leftPath, rightPath), /running results/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

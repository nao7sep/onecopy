import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import test from "node:test";
import { assertPreparedUnchanged, snapshotPreparedGuard } from "./prepared.mjs";
import { runScenarios } from "./scenario-runner.mjs";

const roots = [];
test.afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function temporaryRoot(label) {
  const root = mkdtempSync(join(tmpdir(), `onecopy-ai-runner-${label}-`));
  roots.push(root);
  return root;
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function harness(label, caseIds, fixtureNames = ["fixture.bin"]) {
  const root = temporaryRoot(label);
  const fixtureRoot = join(root, "fixtures");
  mkdirSync(fixtureRoot);
  const bytes = Buffer.from("model-free scenario fixture");
  const fixtures = fixtureNames.map((name) => {
    writeFileSync(join(fixtureRoot, name), bytes);
    return { basename: name, sha256: sha256(bytes), bytes: bytes.length };
  });
  const cases = caseIds.map((id) => ({ id, fixtures, timeoutMs: 10_000 }));
  const parameterPath = join(root, "parameters.json");
  writeFileSync(parameterPath, JSON.stringify({
    schemaVersion: 2,
    profileId: "model-free-runner",
    profileVersion: 1,
    cases,
  }));
  const requirements = [...new Set(caseIds.map((id) => id === "face" ? "face-scoring" : "transcription"))];
  const capabilities = requirements.map((requirement) => ({
    feature: requirement,
    options: [{ id: "none" }],
  }));
  const preparedContext = {
    requirements,
    artifacts: requirements.map((requirement) => ({
      id: `${requirement}-artifact`,
      kind: "model",
      requirements: [requirement],
      readiness: "current",
      identity: { sha256: "a".repeat(64), bytes: 1, version: null },
    })),
    capabilities,
  };
  const managedRoot = join(root, "prepared", "managed");
  const binRoot = join(root, "prepared", "bin");
  mkdirSync(managedRoot, { recursive: true });
  mkdirSync(binRoot, { recursive: true });
  const artifactPaths = preparedContext.artifacts.map(({ id }) => {
    const relativePath = join("models", `${id}.bin`);
    const path = join(managedRoot, relativePath);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, "prepared");
    return { id, relativePath, path };
  });
  const scenarioExecutable = join(binRoot, "onecopy-ai-scenario-test");
  writeFileSync(scenarioExecutable, "scenario");
  const preparedGuard = snapshotPreparedGuard(managedRoot, scenarioExecutable, artifactPaths);
  const loadPrepared = () => ({
    manifest: {
      source: {
        commit: "b".repeat(40),
        dirty: false,
        trackedDiffSha256: "c".repeat(64),
        untrackedCount: 0,
        untrackedContentSha256: "d".repeat(64),
      },
      platform: process.platform,
      architecture: process.arch,
      targetTriple: "test-target",
      toolchain: { rustc: "rustc test", cargo: "cargo test", node: process.version },
      compileFeatures: ["ai-test-support"],
      scenarioExecutable: { basename: "onecopy-ai-scenario-test", sha256: "e".repeat(64) },
    },
    buildManifestSha256: "f".repeat(64),
    scenarioExecutable,
    managedRoot,
    preparedContext,
    preparedGuard,
  });
  return {
    root,
    fixtureRoot,
    parameterPath,
    reportPath: join(root, "report.json"),
    repositoryRoot: root,
    preparedRoot: join(root, "prepared"),
    loadPrepared,
    guardFiles: [scenarioExecutable, ...artifactPaths.map(({ path }) => path)],
  };
}

function terminal(request, { outcome = "passed", observedAcceleration = null } = {}) {
  return {
    schemaVersion: 1,
    scenarioId: request.scenarioId,
    outcome,
    configuredAcceleration: request.configuredAcceleration,
    observedAcceleration,
    correctness: outcome === "passed" ? { ready: request.fixtures.length } : null,
    failure: outcome === "failed" ? { category: "correctness", message: "oracle rejected output" } : null,
    observations: null,
  };
}

function execution(result, overrides = {}) {
  return {
    code: 0,
    timedOut: false,
    interrupted: false,
    stdout: `${JSON.stringify({ event: "scenario-result", result })}\n`,
    stderr: "",
    launchMs: 1,
    wallMs: 2,
    peakProcessTreeBytes: null,
    ...overrides,
  };
}

test("runner maps same-digest fixtures by basename, checkpoints, and cleans scratch", async () => {
  const paths = harness("mapping", ["face"], ["reference.bin", "variation.bin"]);
  let requestPath;
  const runOwned = async (_command, args, options) => {
    [requestPath] = args;
    const partial = JSON.parse(readFileSync(paths.reportPath, "utf8"));
    assert.equal(partial.outcome, "running");
    assert.equal(partial.cases[0].outcome, "running");
    const request = JSON.parse(readFileSync(requestPath, "utf8"));
    assert.deepEqual(request.fixtures.map(({ path }) => basename(path)), ["reference.bin", "variation.bin"]);
    options.onStdout("diagnostic: /Users/private/fixture.bin\n");
    options.onStdout(`${JSON.stringify({ event: "scenario-progress", scenarioId: "face", completed: 1, total: 2 })}\n`);
    return execution(terminal(request));
  };

  const report = await runScenarios({ ...paths, observe: false }, { loadPrepared: paths.loadPrepared, runOwned });
  assert.equal(report.schemaVersion, 3);
  assert.equal(report.outcome, "passed");
  assert.equal(report.cases[0].timeoutMs, 10_000);
  assert.equal(existsSync(dirname(requestPath)), false);
});

test("correctness failures are recorded without preventing independent later cases", async () => {
  const paths = harness("continue", ["audio-transcription", "video-transcription"]);
  let calls = 0;
  const runOwned = async (_command, [requestPath]) => {
    const request = JSON.parse(readFileSync(requestPath, "utf8"));
    calls += 1;
    return execution(terminal(request, { outcome: calls === 1 ? "failed" : "passed" }));
  };

  let guardChecks = 0;
  const report = await runScenarios({ ...paths, observe: false }, {
    loadPrepared: paths.loadPrepared,
    runOwned,
    assertPreparedUnchanged(guard) {
      guardChecks += 1;
      assertPreparedUnchanged(guard);
    },
  });
  assert.equal(calls, 2);
  assert.equal(guardChecks, 2, "unchanged files need one stat-only guard per case");
  assert.equal(report.outcome, "failed");
  assert.deepEqual(report.cases.map(({ outcome }) => outcome), ["failed", "passed"]);
});

test("same-length replacement after preflight blocks the first child", async () => {
  const paths = harness("stale-first", ["face"]);
  const prior = readFileSync(paths.guardFiles[1]);
  writeFileSync(paths.guardFiles[1], Buffer.alloc(prior.length, 0x78));
  let calls = 0;

  const report = await runScenarios({ ...paths, observe: false }, {
    loadPrepared: paths.loadPrepared,
    runOwned: async () => {
      calls += 1;
      throw new Error("stale prepared files must stop before spawn");
    },
  });

  assert.equal(calls, 0);
  assert.equal(report.outcome, "failed");
  assert.equal(report.cases[0].failure.category, "prepared-stale");
});

test("same-length replacement between cases blocks the later child", async () => {
  const paths = harness("stale-between", ["audio-transcription", "video-transcription"]);
  let calls = 0;
  const runOwned = async (_command, [requestPath]) => {
    calls += 1;
    const request = JSON.parse(readFileSync(requestPath, "utf8"));
    if (calls === 1) {
      const prior = readFileSync(paths.guardFiles[1]);
      writeFileSync(paths.guardFiles[1], Buffer.alloc(prior.length, 0x79));
    }
    return execution(terminal(request));
  };

  const report = await runScenarios({ ...paths, observe: false }, {
    loadPrepared: paths.loadPrepared,
    runOwned,
  });

  assert.equal(calls, 1);
  assert.deepEqual(report.cases.map(({ outcome }) => outcome), ["passed", "failed"]);
  assert.equal(report.cases[1].failure.category, "prepared-stale");
});

test("configured and observed accelerator mismatch is an infrastructure stop", async () => {
  const paths = harness("accelerator", ["audio-transcription", "video-transcription"]);
  let calls = 0;
  const runOwned = async (_command, [requestPath]) => {
    calls += 1;
    const request = JSON.parse(readFileSync(requestPath, "utf8"));
    return execution(terminal(request, { observedAcceleration: "metal" }));
  };

  const report = await runScenarios({ ...paths, observe: false }, { loadPrepared: paths.loadPrepared, runOwned });
  assert.equal(calls, 1);
  assert.equal(report.outcome, "failed");
  assert.equal(report.cases[0].failure.category, "scenario-protocol");
});

test("timeouts continue but interruption seals the active case and stops", async () => {
  const paths = harness("bounds", ["face", "audio-transcription", "video-transcription"]);
  let calls = 0;
  const runOwned = async (_command, [requestPath]) => {
    calls += 1;
    const request = JSON.parse(readFileSync(requestPath, "utf8"));
    if (calls === 1) return execution(terminal(request), { timedOut: true });
    return execution(terminal(request), { interrupted: true });
  };

  const report = await runScenarios({ ...paths, observe: false }, { loadPrepared: paths.loadPrepared, runOwned });
  assert.equal(calls, 2);
  assert.equal(report.outcome, "interrupted");
  assert.deepEqual(report.cases.map(({ outcome }) => outcome), ["failed", "interrupted"]);
});

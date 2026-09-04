import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { arch, cpus, platform, release, tmpdir, totalmem } from "node:os";
import { resolve } from "node:path";
import { validateParameters } from "./contracts.mjs";
import { indexFixtureRoot, resolveFixtures } from "./fixtures.mjs";
import { dependenciesForCase, loadPrepared } from "./prepared.mjs";
import { runOwned } from "./process.mjs";
import { assertPrivacySafe, safeFailure, writeAtomicReport } from "./report.mjs";

function machineFacts() {
  return {
    platform: platform(),
    osVersion: release(),
    architecture: arch(),
    cpuModel: cpus()[0]?.model ?? "unknown",
    logicalCpuCount: cpus().length,
    totalMemoryBytes: totalmem(),
  };
}

function parseFinal(stdout) {
  return stdout
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line))
    .findLast((entry) => entry.event === "live-result");
}

function assertOracle(item, result) {
  if (item.id === "face") {
    const ready = result.items.filter(
      ({ score }) => Number.isFinite(score) && score >= item.oracle.minimumScore && score <= item.oracle.maximumScore,
    );
    if (ready.length < item.oracle.minimumReady) {
      throw new Error(`face oracle expected ${item.oracle.minimumReady} ready results, received ${ready.length}`);
    }
    return { ready: ready.length, total: result.items.length };
  }
  if (result.matchedTerms < item.oracle.minimumTermMatches) {
    throw new Error(`transcript oracle matched ${result.matchedTerms} semantic terms`);
  }
  if (item.oracle.rejectPhraseLoop && result.phraseLoop) {
    throw new Error("transcript oracle detected a phrase loop");
  }
  return { matchedTerms: result.matchedTerms, segmentCount: result.segmentCount, phraseLoop: false };
}

export async function runLive({ repositoryRoot, parameterPath, fixtureRoot, preparedRoot, reportPath }) {
  const parameters = validateParameters(JSON.parse(readFileSync(parameterPath, "utf8")));
  const selected = parameters.cases.filter(({ surface }) => surface === "adapter");
  if (selected.length === 0) throw new Error("the parameter set selects no adapter cases");
  const allResolved = resolveFixtures(
    indexFixtureRoot(fixtureRoot),
    selected.flatMap(({ fixtures }) => fixtures),
  );
  const { manifest, driver, managedRoot, preparedContext } = loadPrepared(
    repositoryRoot,
    preparedRoot,
    parameters,
  );
  const report = {
    schemaVersion: 1,
    profileId: parameters.profileId,
    profileVersion: parameters.profileVersion,
    outcome: "running",
    startedAtUtc: new Date().toISOString(),
    machine: machineFacts(),
    source: manifest.source,
    binary: manifest.binary,
    cases: [],
  };
  writeAtomicReport(reportPath, report);
  for (const item of selected) {
    const resolvedFixtures = item.fixtures.map((fixture) =>
      allResolved.find((resolved) => resolved.reference.sha256 === fixture.sha256),
    );
    if (resolvedFixtures.some((fixture) => !fixture)) throw new Error("resolved fixture mapping failed");
    const scratch = item.id === "face"
      ? null
      : mkdtempSync(resolve(tmpdir(), "onecopy-ai-live-"));
    const args = item.id === "face"
      ? ["live-face", managedRoot, ...resolvedFixtures.map(({ path }) => path)]
      : [
          "live-transcription",
          managedRoot,
          scratch,
          item.acceleration,
          resolvedFixtures[0].path,
          ...item.oracle.semanticTerms,
        ];
    const startedAtUtc = new Date().toISOString();
    try {
      const execution = await runOwned(driver, args, {
        cwd: repositoryRoot,
        env: { ONECOPY_AI_OFFLINE: "1" },
        timeoutMs: item.timeoutMs,
      });
      if (execution.interrupted) {
        const error = new Error("live adapter interrupted by operator");
        error.interrupted = true;
        throw error;
      }
      if (execution.code !== 0) throw new Error(execution.timedOut ? "live adapter timed out" : execution.stderr.trim());
      const output = parseFinal(execution.stdout);
      if (!output) throw new Error("live adapter emitted no terminal result");
      const correctness = assertOracle(item, output);
      report.cases.push({
        id: item.id,
        outcome: "passed",
        startedAtUtc,
        finishedAtUtc: new Date().toISOString(),
        dependencies: dependenciesForCase(preparedContext, item),
        fixtures: item.fixtures,
        requestedAcceleration: item.acceleration,
        effectiveAcceleration: output.effectiveAcceleration,
        correctness,
        phases: Object.fromEntries(
          Object.entries(output)
            .filter(([key, value]) => key.endsWith("Ms") && Number.isFinite(value))
            .map(([key, value]) => [key, value]),
        ),
        totalWallMs: execution.wallMs,
        peakProcessTreeBytes: execution.peakProcessTreeBytes,
        ...(output.normalizedOutputSha256 ? { normalizedOutputSha256: output.normalizedOutputSha256 } : {}),
      });
    } catch (error) {
      report.cases.push({
        id: item.id,
        outcome: "failed",
        startedAtUtc,
        finishedAtUtc: new Date().toISOString(),
        dependencies: dependenciesForCase(preparedContext, item),
        fixtures: item.fixtures,
        requestedAcceleration: item.acceleration,
        failure: safeFailure("live-adapter", error),
      });
      report.outcome = error.interrupted ? "interrupted" : "failed";
    } finally {
      if (scratch) rmSync(scratch, { recursive: true, force: true });
    }
    assertPrivacySafe(report);
    writeAtomicReport(reportPath, report);
    if (report.outcome === "interrupted") {
      report.finishedAtUtc = new Date().toISOString();
      writeAtomicReport(reportPath, report);
      return report;
    }
  }
  if (report.outcome === "running") report.outcome = "passed";
  report.finishedAtUtc = new Date().toISOString();
  writeAtomicReport(reportPath, report);
  return report;
}

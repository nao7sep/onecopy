import { cpus, arch, platform, release, totalmem } from "node:os";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { cloneArtifactTree } from "./artifact-tree.mjs";
import { dependencySets, validateParameters } from "./contracts.mjs";
import { indexFixtureRoot, materializeFixtures, resolveFixtures } from "./fixtures.mjs";
import { loadPrepared } from "./prepared.mjs";
import { runOwned } from "./process.mjs";
import { safeFailure, writeAtomicReport } from "./report.mjs";

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

function preflightAcceleration(item, manifest) {
  const feature = item.id === "face" ? "face-scoring" : "transcription";
  const capability = manifest.accelerationCapabilities.find((candidate) => candidate.feature === feature);
  if (!capability?.modes.includes(item.acceleration)) {
    throw new Error(`${feature} acceleration is absent from the prepared binary: ${item.acceleration}`);
  }
}

export async function runBenchmark({ repositoryRoot, parameterPath, fixtureRoot, preparedRoot, reportPath }) {
  const parameters = validateParameters(JSON.parse(readFileSync(parameterPath, "utf8")));
  const selected = parameters.cases.filter(({ surface }) => surface === "app");
  if (selected.length === 0) throw new Error("the parameter set selects no running-app cases");
  const allResolved = resolveFixtures(
    indexFixtureRoot(fixtureRoot),
    selected.flatMap(({ fixtures }) => fixtures),
  );
  const { manifest, binary } = loadPrepared(repositoryRoot, preparedRoot, parameters);
  selected.forEach((item) => preflightAcceleration(item, manifest));

  const runRoot = resolve(
    preparedRoot,
    "runs",
    `${parameters.profileId}-${new Date().toISOString().replace(/[:.]/g, "-")}`,
  );
  mkdirSync(resolve(preparedRoot, "runs"), { recursive: true });
  mkdirSync(runRoot, { recursive: false });
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
    const caseRoot = resolve(runRoot, item.id);
    const home = resolve(caseRoot, "home");
    const source = resolve(caseRoot, "source");
    const caseResultPath = resolve(caseRoot, "case-result.json");
    const timingPath = resolve(caseRoot, "timing.json");
    mkdirSync(home, { recursive: true });
    mkdirSync(source, { recursive: true });
    cloneArtifactTree(resolve(preparedRoot, "managed"), home);
    const resolvedForCase = item.fixtures.map((fixture) =>
      allResolved.find((resolved) => resolved.reference.sha256 === fixture.sha256),
    );
    if (resolvedForCase.some((fixture) => !fixture)) throw new Error("resolved fixture mapping failed");
    materializeFixtures(source, resolvedForCase);
    writeFileSync(
      resolve(home, "config.json"),
      `${JSON.stringify({
        sourceDirs: [source],
        defaultTimezone: "UTC",
        checkSourceFoldersAtLaunch: true,
        checkUpdatesAtLaunch: false,
        keepAwakeDuringIndexing: false,
        videoSnapshotsEnabled: false,
        similarPhotoAnalysisEnabled: false,
        scoreFaces: item.id === "face",
        videoTranscriptionEnabled: item.id === "video-transcription",
        audioTranscriptionEnabled: item.id === "audio-transcription",
        aiAcceleration: {
          transcription: item.id === "face" ? "none" : item.acceleration,
          "face-scoring": "none",
        },
      }, null, 2)}\n`,
    );
    const startedAtUtc = new Date().toISOString();
    try {
      const execution = await runOwned(
        process.execPath,
        [resolve(repositoryRoot, "node_modules", "@wdio", "cli", "bin", "wdio.js"), "run", "wdio.conf.mjs"],
        {
          cwd: repositoryRoot,
          timeoutMs: item.timeoutMs,
          env: {
            ONECOPY_AI_OFFLINE: "1",
            ONECOPY_AI_BENCHMARK_CASE: JSON.stringify(item),
            ONECOPY_AI_CASE_TIMEOUT_MS: String(item.timeoutMs),
            ONECOPY_AI_CASE_RESULT: caseResultPath,
            ONECOPY_AI_TIMING_FILE: timingPath,
            ONECOPY_E2E_BINARY: binary,
            ONECOPY_E2E_HOME: home,
          },
        },
      );
      if (execution.interrupted) {
        const error = new Error("running-app benchmark interrupted by operator");
        error.interrupted = true;
        throw error;
      }
      if (execution.code !== 0) {
        throw new Error(execution.timedOut ? "running-app case timed out" : execution.stderr.trim());
      }
      const result = JSON.parse(readFileSync(caseResultPath, "utf8"));
      const instrumentation = JSON.parse(readFileSync(timingPath, "utf8"));
      if (instrumentation.requestedAcceleration !== item.acceleration ||
          instrumentation.effectiveAcceleration !== item.acceleration) {
        throw new Error("requested and effective acceleration differ");
      }
      report.cases.push({
        id: item.id,
        outcome: "passed",
        startedAtUtc,
        finishedAtUtc: new Date().toISOString(),
        dependencies: manifest.dependencies.filter(({ id }) => dependencySets[item.id].includes(id)),
        fixtures: item.fixtures,
        requestedAcceleration: item.acceleration,
        effectiveAcceleration: instrumentation.effectiveAcceleration,
        correctness: result.correctness,
        phases: {
          ...result.phases,
          engine: instrumentation.events,
        },
        totalWallMs: execution.wallMs,
        peakProcessTreeBytes: execution.peakProcessTreeBytes,
        ...(result.normalizedOutputSha256 ? { normalizedOutputSha256: result.normalizedOutputSha256 } : {}),
      });
    } catch (error) {
      report.cases.push({
        id: item.id,
        outcome: "failed",
        startedAtUtc,
        finishedAtUtc: new Date().toISOString(),
        dependencies: manifest.dependencies.filter(({ id }) => dependencySets[item.id].includes(id)),
        fixtures: item.fixtures,
        requestedAcceleration: item.acceleration,
        failure: safeFailure("running-app", error),
      });
      report.outcome = error.interrupted ? "interrupted" : "failed";
    }
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

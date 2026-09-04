import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { arch, cpus, platform, release, tmpdir, totalmem } from "node:os";
import { resolve } from "node:path";
import { validateParameters, validateResult } from "./contracts.mjs";
import { indexFixtureRoot, materializeFixtures, resolveFixtures } from "./fixtures.mjs";
import { dependenciesForCase, loadPrepared } from "./prepared.mjs";
import { runOwned } from "./process.mjs";
import { jsonLineEvents } from "./progress.mjs";
import { writeAtomicReport } from "./report.mjs";

function machineFacts() {
  const processors = cpus();
  return {
    platform: platform(),
    osVersion: release(),
    architecture: arch(),
    cpuModel: processors[0]?.model ?? "unknown",
    logicalCpuCount: Math.max(1, processors.length),
    totalMemoryBytes: totalmem(),
  };
}

function terminalResult(stdout) {
  for (const line of stdout.split(/\r?\n/).filter(Boolean).reverse()) {
    try {
      const event = JSON.parse(line);
      if (event?.event === "scenario-result") return event.result;
    } catch {
      // Only the structured terminal event is evidence; incidental process
      // output remains diagnostic-only and never enters the report.
    }
  }
  return undefined;
}

function validScenarioTerminal(terminal, item, observe) {
  if (!terminal || terminal.schemaVersion !== 1 || terminal.scenarioId !== item.id ||
      !["passed", "failed"].includes(terminal.outcome) ||
      terminal.configuredAcceleration !== item.acceleration) {
    return false;
  }
  const allowedAcceleration = item.id === "face" ? ["none"] : ["none", "metal"];
  if (terminal.observedAcceleration !== null &&
      (!allowedAcceleration.includes(terminal.observedAcceleration) ||
       terminal.observedAcceleration !== item.acceleration)) {
    return false;
  }
  const correctness = terminal.correctness;
  const failure = terminal.failure;
  if (terminal.outcome === "passed") {
    if (!correctness || typeof correctness !== "object" || Array.isArray(correctness) ||
        failure !== null) return false;
  } else if (!failure || typeof failure.category !== "string" || failure.category.trim() === "" ||
             typeof failure.message !== "string" || failure.message.trim() === "" ||
             correctness !== null) {
    return false;
  }
  if (!observe) return terminal.observations === null;
  return terminal.observations && Array.isArray(terminal.observations.phases) &&
    terminal.observations.phases.every((event) =>
      event && typeof event.phase === "string" && event.phase.trim() !== "" &&
      Number.isFinite(event.wallMs) && event.wallMs >= 0);
}

function fixedFailure(category, message) {
  return { category, message };
}

function finishCase(caseResult, outcome, fields = {}) {
  Object.assign(caseResult, fields, {
    outcome,
    finishedAtUtc: new Date().toISOString(),
  });
}

function elapsedMs(started) {
  return Number(process.hrtime.bigint() - started) / 1_000_000;
}

export async function runScenarios({
  repositoryRoot,
  parameterPath,
  fixtureRoot,
  preparedRoot,
  reportPath,
  observe,
}, edges = {}) {
  const parameters = validateParameters(JSON.parse(readFileSync(parameterPath, "utf8")));
  const allResolved = resolveFixtures(
    indexFixtureRoot(fixtureRoot),
    parameters.cases.flatMap(({ fixtures }) => fixtures),
  );
  const {
    manifest,
    buildManifestSha256,
    scenarioExecutable,
    managedRoot,
    preparedContext,
  } = (edges.loadPrepared ?? loadPrepared)(repositoryRoot, preparedRoot, parameters);
  const executeScenario = edges.runOwned ?? runOwned;
  const capabilities = preparedContext.capabilities.map(({ feature, options }) => ({
    feature,
    options: options.map(({ id }) => id),
  }));
  const report = {
    schemaVersion: 2,
    profileId: parameters.profileId,
    profileVersion: parameters.profileVersion,
    mode: observe ? "benchmark" : "live",
    outcome: "running",
    startedAtUtc: new Date().toISOString(),
    machine: machineFacts(),
    source: manifest.source,
    executable: manifest.scenarioExecutable,
    buildManifestSha256,
    build: {
      platform: manifest.platform,
      architecture: manifest.architecture,
      targetTriple: manifest.targetTriple,
      toolchain: manifest.toolchain,
      compileFeatures: manifest.compileFeatures,
      capabilities,
    },
    cases: [],
  };
  const persist = () => writeAtomicReport(reportPath, validateResult(report));
  persist();

  for (const item of parameters.cases) {
    const runnerPhases = [];
    const caseResult = {
      scenarioId: item.id,
      timeoutMs: item.timeoutMs,
      outcome: "running",
      startedAtUtc: new Date().toISOString(),
      dependencies: dependenciesForCase(preparedContext, item),
      fixtures: item.fixtures,
      configuredAcceleration: item.acceleration,
      observedAcceleration: null,
      observations: null,
    };
    report.cases.push(caseResult);
    persist();
    const caseStarted = process.hrtime.bigint();

    let scratch;
    let stopRemaining = false;
    let terminalWallMs;
    try {
      scratch = mkdtempSync(resolve(tmpdir(), "onecopy-ai-scenario-"));
      const materializationStarted = process.hrtime.bigint();
      const resolvedForCase = item.fixtures.map((fixture) =>
        allResolved.find(({ reference }) =>
          reference.basename === fixture.basename && reference.sha256 === fixture.sha256),
      );
      if (resolvedForCase.some((fixture) => !fixture)) {
        throw new Error("resolved fixture mapping failed");
      }
      const fixturePaths = materializeFixtures(resolve(scratch, "source"), resolvedForCase);
      const requestPath = resolve(scratch, "request.json");
      writeFileSync(requestPath, `${JSON.stringify({
        schemaVersion: 1,
        scenarioId: item.id,
        managedRoot,
        scratchRoot: resolve(scratch, "operation"),
        configuredAcceleration: item.acceleration,
        observe,
        fixtures: item.fixtures.map((fixture, index) => ({
          ...fixture,
          path: fixturePaths[index],
        })),
      })}\n`);
      runnerPhases.push({ phase: "input-materialization", wallMs: elapsedMs(materializationStarted) });
      const progress = jsonLineEvents((event) => {
        if (event?.event !== "scenario-progress" || event.scenarioId !== item.id) return;
        if (Number.isSafeInteger(event.percent)) {
          process.stdout.write(`${item.id}: ${Math.max(0, Math.min(100, event.percent))}%\n`);
        } else if (Number.isSafeInteger(event.completed) && Number.isSafeInteger(event.total) &&
                   event.total > 0) {
          process.stdout.write(`${item.id}: ${event.completed}/${event.total}\n`);
        }
      });
      const execution = await executeScenario(scenarioExecutable, [requestPath], {
        cwd: repositoryRoot,
        env: { ONECOPY_AI_OFFLINE: "1" },
        timeoutMs: item.timeoutMs,
        measureMemory: observe,
        onStdout: progress.push,
      });
      progress.finish();
      runnerPhases.push({ phase: "process-launch", wallMs: execution.launchMs });
      if (execution.interrupted) {
        finishCase(caseResult, "interrupted", {
          failure: fixedFailure("interrupted", "scenario execution was interrupted"),
        });
      } else if (execution.timedOut) {
        finishCase(caseResult, "failed", {
          failure: fixedFailure("timeout", "scenario execution exceeded its operational bound"),
        });
      } else if (execution.code !== 0) {
        stopRemaining = true;
        finishCase(caseResult, "failed", {
          failure: fixedFailure("scenario-process", "scenario process did not complete"),
        });
      } else {
        const readbackStarted = process.hrtime.bigint();
        const terminal = terminalResult(execution.stdout);
        runnerPhases.push({ phase: "protocol-readback", wallMs: elapsedMs(readbackStarted) });
        if (!validScenarioTerminal(terminal, item, observe)) {
          stopRemaining = true;
          finishCase(caseResult, "failed", {
            failure: fixedFailure("scenario-protocol", "scenario process returned no valid terminal result"),
          });
        } else {
          finishCase(caseResult, terminal.outcome, {
            observedAcceleration: terminal.observedAcceleration ?? null,
            ...(terminal.correctness ? { correctness: terminal.correctness } : {}),
            ...(terminal.failure ? { failure: terminal.failure } : {}),
            observations: observe
              ? {
                  wallMs: execution.wallMs,
                  peakProcessTreeBytes: execution.peakProcessTreeBytes,
                  phases: [...runnerPhases, ...(terminal.observations?.phases ?? [])],
                }
              : null,
          });
        }
      }
    } catch {
      stopRemaining = true;
      finishCase(caseResult, "failed", {
        failure: fixedFailure("scenario-runner", "scenario runner could not finish this case"),
      });
    } finally {
      terminalWallMs = elapsedMs(caseStarted);
      if (scratch) rmSync(scratch, { recursive: true, force: true });
    }
    if (observe) {
      const processObservations = caseResult.observations;
      caseResult.observations = {
        wallMs: terminalWallMs,
        peakProcessTreeBytes: processObservations?.peakProcessTreeBytes ?? null,
        phases: processObservations?.phases ?? runnerPhases,
      };
    }
    persist();
    if (caseResult.outcome === "interrupted" || stopRemaining) break;
  }

  report.outcome = report.cases.some(({ outcome }) => outcome === "interrupted")
    ? "interrupted"
    : report.cases.every(({ outcome }) => outcome === "passed")
      ? "passed"
      : "failed";
  report.finishedAtUtc = new Date().toISOString();
  persist();
  return report;
}

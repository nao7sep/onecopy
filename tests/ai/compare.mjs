import { readFileSync } from "node:fs";
import { validateResult } from "./contracts.mjs";
import { compatibleResults } from "./report.mjs";

export function compare(leftPath, rightPath) {
  const left = validateResult(JSON.parse(readFileSync(leftPath, "utf8")));
  const right = validateResult(JSON.parse(readFileSync(rightPath, "utf8")));
  if (left.outcome === "running" || right.outcome === "running") {
    throw new Error("running results cannot be compared");
  }
  const { machineFactsMatch } = compatibleResults(left, right);
  const cases = left.cases.map((leftCase, index) => {
    const rightCase = right.cases[index];
    if (leftCase.scenarioId !== rightCase.scenarioId) throw new Error("result scenario order differs");
    if (leftCase.outcome !== "passed" || rightCase.outcome !== "passed") {
      return {
        scenarioId: leftCase.scenarioId,
        correctnessEquivalent: false,
        performanceComparable: false,
        reason: "one or both scenarios did not pass",
      };
    }
    const numericPhases = (observations) => {
      const values = {};
      for (const event of observations?.phases ?? []) {
        if (!Number.isFinite(event.wallMs)) continue;
        values[event.phase] = (values[event.phase] ?? 0) + event.wallMs;
      }
      return values;
    };
    const leftPhases = numericPhases(leftCase.observations);
    const rightPhases = numericPhases(rightCase.observations);
    const phaseTimeRatios = Object.fromEntries(
      [...new Set([...Object.keys(leftPhases), ...Object.keys(rightPhases)])]
        .map((phase) => [
          phase,
          machineFactsMatch && Number.isFinite(leftPhases[phase]) &&
            Number.isFinite(rightPhases[phase]) && rightPhases[phase] > 0
            ? leftPhases[phase] / rightPhases[phase]
            : null,
        ]),
    );
    const leftWall = leftCase.observations?.wallMs;
    const rightWall = rightCase.observations?.wallMs;
    const hasWall = Number.isFinite(leftWall) && Number.isFinite(rightWall) && rightWall > 0;
    const leftMemory = leftCase.observations?.peakProcessTreeBytes;
    const rightMemory = rightCase.observations?.peakProcessTreeBytes;
    const hasMemory = Number.isFinite(leftMemory) && Number.isFinite(rightMemory);
    const performanceComparable = machineFactsMatch && hasWall;
    return {
      scenarioId: leftCase.scenarioId,
      performanceComparable,
      ...(performanceComparable ? {} : {
        reason: machineFactsMatch ? "measurement observations are unavailable" : "machine facts differ",
      }),
      leftConfiguredAcceleration: leftCase.configuredAcceleration,
      rightConfiguredAcceleration: rightCase.configuredAcceleration,
      leftObservedAcceleration: leftCase.observedAcceleration,
      rightObservedAcceleration: rightCase.observedAcceleration,
      correctnessEquivalent:
        JSON.stringify(leftCase.correctness) === JSON.stringify(rightCase.correctness),
      wallTimeRatio: performanceComparable ? leftWall / rightWall : null,
      phaseTimeRatios,
      peakMemoryDifferenceBytes: machineFactsMatch && hasMemory ? rightMemory - leftMemory : null,
    };
  });
  return {
    profileId: left.profileId,
    profileVersion: left.profileVersion,
    machineFactsMatch,
    crossMachineUse: machineFactsMatch ? null : "descriptive-only",
    cases,
  };
}

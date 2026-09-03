import { readFileSync } from "node:fs";
import { compatibleResults } from "./report.mjs";

export function compare(leftPath, rightPath) {
  const left = JSON.parse(readFileSync(leftPath, "utf8"));
  const right = JSON.parse(readFileSync(rightPath, "utf8"));
  compatibleResults(left, right);
  const cases = left.cases.map((leftCase, index) => {
    const rightCase = right.cases[index];
    if (leftCase.id !== rightCase.id) throw new Error("result case order differs");
    if (leftCase.outcome !== "passed" || rightCase.outcome !== "passed") {
      return { id: leftCase.id, comparable: false, reason: "one or both cases did not pass" };
    }
    const numericPhases = (phases) =>
      Object.fromEntries(
        Object.entries(phases ?? {}).filter(([, value]) => Number.isFinite(value)),
      );
    const leftPhases = numericPhases(leftCase.phases);
    const rightPhases = numericPhases(rightCase.phases);
    const phaseTimeRatios = Object.fromEntries(
      Object.keys(leftPhases)
        .filter((phase) => Number.isFinite(rightPhases[phase]) && rightPhases[phase] > 0)
        .map((phase) => [phase, leftPhases[phase] / rightPhases[phase]]),
    );
    return {
      id: leftCase.id,
      comparable: true,
      leftAcceleration: leftCase.effectiveAcceleration,
      rightAcceleration: rightCase.effectiveAcceleration,
      wallTimeRatio: leftCase.totalWallMs / rightCase.totalWallMs,
      phaseTimeRatios,
      peakMemoryDifferenceBytes: rightCase.peakProcessTreeBytes - leftCase.peakProcessTreeBytes,
    };
  });
  return { profileId: left.profileId, profileVersion: left.profileVersion, cases };
}

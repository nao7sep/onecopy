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
    const numericPhases = (phases) => {
      const values = Object.fromEntries(
        Object.entries(phases ?? {}).filter(([, value]) => Number.isFinite(value)),
      );
      for (const event of phases?.engine ?? []) {
        if (!Number.isFinite(event.wallMs)) continue;
        const key = `engine.${event.feature}.${event.phase}`;
        values[key] = (values[key] ?? 0) + event.wallMs;
      }
      return values;
    };
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
      correctnessEquivalent:
        JSON.stringify(leftCase.correctness) === JSON.stringify(rightCase.correctness),
      normalizedOutputEquivalent:
        leftCase.normalizedOutputSha256 === rightCase.normalizedOutputSha256,
      wallTimeRatio: leftCase.totalWallMs / rightCase.totalWallMs,
      phaseTimeRatios,
      peakMemoryDifferenceBytes: rightCase.peakProcessTreeBytes - leftCase.peakProcessTreeBytes,
    };
  });
  return { profileId: left.profileId, profileVersion: left.profileVersion, cases };
}

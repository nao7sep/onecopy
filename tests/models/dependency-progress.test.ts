import { describe, expect, it } from "vitest";
import {
  managedInstallActivityLine,
  managedInstallLine,
} from "../../src/models/dependencyProgress";
import { inEnglish, number, percent } from "../helpers/i18n";

describe("managed dependency progress", () => {
  it("renders measurable download and checksum byte progress", () => {
    expect(
      inEnglish(managedInstallLine({
        phase: "download",
        done: 1_048_576,
        total: 4_194_304,
        nextPhase: "verify",
      }, number, percent)),
    ).toBe("Downloading — 1 MB / 4 MB (25%)");
    expect(
      inEnglish(managedInstallLine({
        phase: "verify",
        done: 4_194_304,
        total: 4_194_304,
        nextPhase: "install",
      }, number, percent)),
    ).toBe("Verifying — 4 MB / 4 MB (100%)");
  });

  it("keeps unknown server lengths honest and hides meaningless fixed counts", () => {
    expect(
      inEnglish(managedInstallLine({
        phase: "download",
        done: 2_097_152,
        total: null,
        nextPhase: "verify",
      }, number, percent)),
    ).toBe("Downloading — 2 MB");
    expect(
      inEnglish(managedInstallLine({
        phase: "resolve",
        done: 1,
        total: 1,
        nextPhase: "download",
      }, number, percent)),
    ).toBe("Resolving");
  });

  it("presents starting and cancellation without manufacturing progress", () => {
    expect(
      inEnglish(managedInstallActivityLine({ progress: null, cancelling: false }, number, percent)),
    ).toBe("Starting…");
    expect(
      inEnglish(managedInstallActivityLine({ progress: null, cancelling: true }, number, percent)),
    ).toBe("Cancelling…");
  });
});

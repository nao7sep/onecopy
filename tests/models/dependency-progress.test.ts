import { describe, expect, it } from "vitest";
import {
  managedInstallActivityLine,
  managedInstallLine,
} from "../../src/models/dependencyProgress";

describe("managed dependency progress", () => {
  it("renders measurable download and checksum byte progress", () => {
    expect(
      managedInstallLine({
        phase: "download",
        done: 1_048_576,
        total: 4_194_304,
        nextPhase: "verify",
      }),
    ).toBe("Downloading — 1 MB / 4 MB (25%)");
    expect(
      managedInstallLine({
        phase: "verify",
        done: 4_194_304,
        total: 4_194_304,
        nextPhase: "install",
      }),
    ).toBe("Verifying — 4 MB / 4 MB (100%) · Next: Installing");
  });

  it("keeps unknown server lengths honest and fixed phases stable", () => {
    expect(
      managedInstallLine({
        phase: "download",
        done: 2_097_152,
        total: null,
        nextPhase: "verify",
      }),
    ).toBe("Downloading — 2 MB");
    expect(
      managedInstallLine({
        phase: "resolve",
        done: 1,
        total: 1,
        nextPhase: "download",
      }),
    ).toBe("Resolving — 1/1 · Next: Downloading");
  });

  it("presents starting and cancellation without manufacturing progress", () => {
    expect(
      managedInstallActivityLine({ progress: null, cancelling: false }),
    ).toBe("Starting…");
    expect(
      managedInstallActivityLine({ progress: null, cancelling: true }),
    ).toBe("Cancelling…");
  });
});

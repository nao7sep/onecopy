import { describe, expect, it } from "vitest";
import { optionalFeatureSetup } from "../../src/models/optionalFeatures";
import { effectiveConfig } from "../helpers/config";

describe("optional feature setup", () => {
  it("starts every optional feature on for a first setup, as the core's defaults say", () => {
    expect(Object.values(optionalFeatureSetup(effectiveConfig()))).toEqual([
      true,
      true,
      true,
      true,
      true,
    ]);
  });

  it("does not require a managed-tool inventory to choose defaults", () => {
    expect(optionalFeatureSetup(effectiveConfig())).toEqual({
      videoSnapshotsEnabled: true,
      similarPhotoAnalysisEnabled: true,
      scoreFaces: true,
      videoTranscriptionEnabled: true,
      audioTranscriptionEnabled: true,
    });
  });

  it("preserves saved choices when setup is reopened", () => {
    const result = optionalFeatureSetup(effectiveConfig({
      similarPhotoAnalysisEnabled: false,
      videoTranscriptionEnabled: true,
    }));
    expect(result.similarPhotoAnalysisEnabled).toBe(false);
    expect(result.videoTranscriptionEnabled).toBe(true);
  });
});

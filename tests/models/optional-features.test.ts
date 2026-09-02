import { describe, expect, it } from "vitest";
import { optionalFeatureSetup } from "../../src/models/optionalFeatures";

const installed = (id: string) => ({ id, status: "up-to-date" });

describe("optional feature setup", () => {
  it("starts every runnable feature on for a first setup", () => {
    const result = optionalFeatureSetup(
      null,
      [
        installed("ffmpeg"),
        installed("whisper-large-v3-turbo"),
        installed("ultraface-rfb640"),
        installed("hsemotion-enet-b2"),
      ],
      true,
    );
    expect(Object.values(result.choices)).toEqual([true, true, true, true, true]);
    expect(result.reasons).toEqual({});
  });

  it("starts only unavailable first-run choices off and explains why", () => {
    const result = optionalFeatureSetup(null, [], true);
    expect(result.choices).toEqual({
      videoSnapshotsEnabled: false,
      similarPhotoAnalysisEnabled: true,
      scoreFaces: false,
      videoTranscriptionEnabled: false,
      audioTranscriptionEnabled: false,
    });
    expect(result.reasons.videoSnapshotsEnabled).toContain("ffmpeg");
    expect(result.reasons.scoreFaces).toContain("face models");
    expect(result.reasons.audioTranscriptionEnabled).toContain("transcription model");
  });

  it("preserves saved choices when setup is reopened", () => {
    const result = optionalFeatureSetup(
      { similarPhotoAnalysisEnabled: false, videoTranscriptionEnabled: true },
      [],
      false,
    );
    expect(result.choices.similarPhotoAnalysisEnabled).toBe(false);
    expect(result.choices.videoTranscriptionEnabled).toBe(true);
  });
});

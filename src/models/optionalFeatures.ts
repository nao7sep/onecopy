export type OptionalFeatureId =
  | "videoSnapshotsEnabled"
  | "similarPhotoAnalysisEnabled"
  | "scoreFaces"
  | "videoTranscriptionEnabled"
  | "audioTranscriptionEnabled";

export type OptionalFeatureChoices = Record<OptionalFeatureId, boolean>;

export function optionalFeatureSetup(
  config: Record<string, unknown> | null,
): OptionalFeatureChoices {
  const configured = (id: OptionalFeatureId) => config?.[id] !== false;
  return {
    videoSnapshotsEnabled: configured("videoSnapshotsEnabled"),
    similarPhotoAnalysisEnabled: configured("similarPhotoAnalysisEnabled"),
    scoreFaces: configured("scoreFaces"),
    videoTranscriptionEnabled: configured("videoTranscriptionEnabled"),
    audioTranscriptionEnabled: configured("audioTranscriptionEnabled"),
  };
}

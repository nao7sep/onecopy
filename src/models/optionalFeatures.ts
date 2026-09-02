export type OptionalFeatureId =
  | "videoSnapshotsEnabled"
  | "similarPhotoAnalysisEnabled"
  | "scoreFaces"
  | "videoTranscriptionEnabled"
  | "audioTranscriptionEnabled";

export type OptionalFeatureChoices = Record<OptionalFeatureId, boolean>;
export type OptionalFeatureReasons = Partial<Record<OptionalFeatureId, string>>;

interface ToolState {
  id: string;
  status: string;
}

function installed(tools: readonly ToolState[], id: string): boolean {
  return tools.some((tool) => tool.id === id && tool.status !== "not-installed");
}

export function optionalFeatureSetup(
  config: Record<string, unknown> | null,
  tools: readonly ToolState[],
  firstRun: boolean,
): { choices: OptionalFeatureChoices; reasons: OptionalFeatureReasons } {
  const ffmpeg = installed(tools, "ffmpeg");
  const transcriptionModel = installed(tools, "whisper-large-v3-turbo");
  const faceModels =
    installed(tools, "ultraface-rfb640") && installed(tools, "hsemotion-enet-b2");
  const reasons: OptionalFeatureReasons = {};
  if (!ffmpeg) reasons.videoSnapshotsEnabled = "ffmpeg is not installed";
  if (!faceModels) reasons.scoreFaces = "the two face models are not installed";
  if (!ffmpeg || !transcriptionModel) {
    const reason = !ffmpeg && !transcriptionModel
      ? "ffmpeg and the transcription model are not installed"
      : !ffmpeg
        ? "ffmpeg is not installed"
        : "the transcription model is not installed";
    reasons.videoTranscriptionEnabled = reason;
    reasons.audioTranscriptionEnabled = reason;
  }

  const configured = (id: OptionalFeatureId) => config?.[id] !== false;
  const choice = (id: OptionalFeatureId) =>
    configured(id) && !(firstRun && reasons[id] !== undefined);
  return {
    choices: {
      videoSnapshotsEnabled: choice("videoSnapshotsEnabled"),
      similarPhotoAnalysisEnabled: choice("similarPhotoAnalysisEnabled"),
      scoreFaces: choice("scoreFaces"),
      videoTranscriptionEnabled: choice("videoTranscriptionEnabled"),
      audioTranscriptionEnabled: choice("audioTranscriptionEnabled"),
    },
    reasons,
  };
}

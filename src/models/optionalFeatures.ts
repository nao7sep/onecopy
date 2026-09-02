export type OptionalFeatureId =
  | "videoSnapshotsEnabled"
  | "similarPhotoAnalysisEnabled"
  | "scoreFaces"
  | "videoTranscriptionEnabled"
  | "audioTranscriptionEnabled";

export type OptionalFeatureChoices = Record<OptionalFeatureId, boolean>;
export type OptionalFeatureReasons = Partial<Record<OptionalFeatureId, string>>;

export interface OptionalFeatureSupport {
  faceScoring: boolean;
  transcription: boolean;
}

export const NO_OPTIONAL_ANALYSIS_SUPPORT: OptionalFeatureSupport = {
  faceScoring: false,
  transcription: false,
};

export function optionalFeatureSupported(
  id: OptionalFeatureId,
  support: OptionalFeatureSupport,
): boolean {
  if (id === "scoreFaces") return support.faceScoring;
  if (id === "videoTranscriptionEnabled" || id === "audioTranscriptionEnabled") {
    return support.transcription;
  }
  return true;
}

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
  support: OptionalFeatureSupport,
): { choices: OptionalFeatureChoices; reasons: OptionalFeatureReasons } {
  const ffmpeg = installed(tools, "ffmpeg");
  const transcriptionModel = installed(tools, "whisper-large-v3-turbo");
  const faceModels =
    installed(tools, "ultraface-rfb640") && installed(tools, "hsemotion-enet-b2");
  const reasons: OptionalFeatureReasons = {};
  if (!ffmpeg) reasons.videoSnapshotsEnabled = "ffmpeg is not installed";
  if (!support.faceScoring) {
    reasons.scoreFaces = "Currently available only on Apple silicon Macs";
  } else if (!faceModels) {
    reasons.scoreFaces = "the two face models are not installed";
  }
  if (!support.transcription) {
    reasons.videoTranscriptionEnabled = "Currently available only on Apple silicon Macs";
    reasons.audioTranscriptionEnabled = "Currently available only on Apple silicon Macs";
  } else if (!ffmpeg || !transcriptionModel) {
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
    optionalFeatureSupported(id, support) &&
    configured(id) &&
    !(firstRun && reasons[id] !== undefined);
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

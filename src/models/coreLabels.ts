import type { MessageKey } from "../i18n/catalogues";

// The core names its managed tools, AI features and acceleration modes by id;
// the interface says each one in the reader's language. An id without a key
// here falls back to the core's own English label, so a new tool still shows.
const TOOL_KEYS: Record<string, MessageKey> = {
  ffmpeg: "tool.ffmpeg",
  "onnxruntime-win-x64": "tool.onnxruntimeWinX64",
  "whisper-large-v3-turbo": "tool.whisperLargeV3Turbo",
  "ultraface-rfb640": "tool.ultrafaceRfb640",
  "hsemotion-enet-b2": "tool.hsemotionEnetB2",
};

const FEATURE_KEYS: Record<string, MessageKey> = {
  transcription: "acceleration.transcription",
  "face-scoring": "acceleration.faceScoring",
};

const MODE_KEYS: Record<string, MessageKey> = {
  none: "acceleration.modeNone",
  metal: "acceleration.modeMetal",
};

function labelled(
  keys: Record<string, MessageKey>,
  id: string,
  label: string,
  t: (key: MessageKey) => string,
): string {
  const key = keys[id];
  return key === undefined ? label : t(key);
}

export function toolLabel(id: string, label: string, t: (key: MessageKey) => string): string {
  return labelled(TOOL_KEYS, id, label, t);
}

export function featureLabel(id: string, label: string, t: (key: MessageKey) => string): string {
  return labelled(FEATURE_KEYS, id, label, t);
}

export function accelerationModeLabel(
  id: string,
  label: string,
  t: (key: MessageKey) => string,
): string {
  return labelled(MODE_KEYS, id, label, t);
}

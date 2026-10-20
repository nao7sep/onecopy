import type { MessageKey } from "../i18n/catalogues";
import { message, type MessageValue, type MessageValues, type Translator } from "../i18n/translate";
import type { DependencyState } from "../state/binaries-store";

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

/** What a tool entry is called: its id, the core's English label, and the
 * upstream release number the label names where the pin has one. */
export type ToolIdentity = Pick<DependencyState, "id" | "label" | "upstreamVersion">;

function toolValues(tool: ToolIdentity): MessageValues | undefined {
  return tool.upstreamVersion === null ? undefined : { version: tool.upstreamVersion };
}

export function toolLabel(tool: ToolIdentity, t: Translator["t"]): string {
  const key = TOOL_KEYS[tool.id];
  return key === undefined ? tool.label : t(key, toolValues(tool));
}

/** The same name as a message value, for text kept in state. */
export function toolName(tool: ToolIdentity): MessageValue {
  const key = TOOL_KEYS[tool.id];
  return key === undefined ? tool.label : message(key, toolValues(tool));
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

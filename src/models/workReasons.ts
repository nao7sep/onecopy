import type { MessageKey } from "../i18n/catalogues";

// The core names the condition that holds work back; the interface says it in
// the reader's language. A code the core adds without a key here shows as the
// code, which the catalogue gate and the on-screen-key check both catch.
const REASON_KEYS: Record<string, MessageKey> = {
  "waiting-for-ffmpeg": "reason.waitingForFfmpeg",
  "waiting-for-face-models": "reason.waitingForFaceModels",
  "waiting-for-transcription-model": "reason.waitingForTranscriptionModel",
  "enable-video-snapshots": "reason.enableVideoSnapshots",
  "enable-similarity": "reason.enableSimilarity",
  "enable-face-scoring": "reason.enableFaceScoring",
  "enable-video-transcription": "reason.enableVideoTranscription",
  "enable-audio-transcription": "reason.enableAudioTranscription",
  "recheck-section": "reason.recheckSection",
  "waiting-for-indexing": "reason.waitingForIndexing",
};

export function workReasonKey(reason: string | null | undefined): MessageKey | null {
  if (reason === null || reason === undefined) return null;
  return REASON_KEYS[reason] ?? null;
}

// The words to show for a core-supplied reason: the catalogue's sentence when
// the core named a condition, otherwise whatever it recorded, as recorded.
export function reasonText(
  reason: string | null | undefined,
  t: (key: MessageKey) => string,
): string | null {
  if (reason === null || reason === undefined) return null;
  const key = workReasonKey(reason);
  return key === null ? reason : t(key);
}

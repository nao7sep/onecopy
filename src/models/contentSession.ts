import type { PlaybackMedium } from "./playback";
import { keyOf } from "./items";

export interface ContentSessionState {
  textWrap: boolean;
  textEncodings: Record<string, string>;
  transcriptOpen: Record<PlaybackMedium, boolean>;
  transcriptViews: Record<string, TranscriptViewState>;
}

export interface TranscriptViewState {
  scrollTop: number;
  selection: [number, number] | null;
}

export function textEncodingKey(hash: string | null, pathId: number | null): string {
  return hash ?? (pathId === null ? "missing" : keyOf(null, pathId));
}

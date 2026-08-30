import { emit, listen } from "@tauri-apps/api/event";
import type { PlaybackMedium } from "../models/playback";
import type { ContentSessionState, TranscriptViewState } from "../models/contentSession";
import type { PlaybackSession } from "../models/playback";
import { log, toErrorFields } from "../repositories";

let state: ContentSessionState = {
  textWrap: true,
  textEncodings: {},
  transcriptOpen: { video: false, audio: true },
  transcriptViews: {},
};
let installed = false;
let mediaKey: string | null = null;

function broadcast(): void {
  void emit("content-session://state", state).catch((error) => {
    log.error("content session broadcast failed", toErrorFields(error));
  });
}

/** Main-webview owner for cross-window, session-only presentation choices. */
export async function installContentSessionWorkflow(): Promise<void> {
  if (installed) return;
  installed = true;
  await Promise.all([
    listen<{ wrap: boolean }>("content-session://set-text-wrap", ({ payload }) => {
      state = { ...state, textWrap: payload.wrap };
      broadcast();
    }),
    listen<{ key: string; encoding: string }>(
      "content-session://set-text-encoding",
      ({ payload }) => {
        state = {
          ...state,
          textEncodings: { ...state.textEncodings, [payload.key]: payload.encoding },
        };
        broadcast();
      },
    ),
    listen<{ medium: PlaybackMedium; open: boolean }>(
      "content-session://set-transcript-open",
      ({ payload }) => {
        state = {
          ...state,
          transcriptOpen: { ...state.transcriptOpen, [payload.medium]: payload.open },
        };
        broadcast();
      },
    ),
    listen<{ key: string; view: TranscriptViewState }>(
      "content-session://set-transcript-view",
      ({ payload }) => {
        state = {
          ...state,
          transcriptViews: { ...state.transcriptViews, [payload.key]: payload.view },
        };
        broadcast();
      },
    ),
    listen<PlaybackSession | null>("playback://state", ({ payload }) => {
      const nextKey = payload?.key ?? null;
      if (nextKey === mediaKey) return;
      mediaKey = nextKey;
      if (Object.keys(state.transcriptViews).length === 0) return;
      state = { ...state, transcriptViews: {} };
      broadcast();
    }),
    listen("content-session://client-ready", broadcast),
  ]);
  broadcast();
}

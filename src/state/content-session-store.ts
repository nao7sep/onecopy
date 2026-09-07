import { create } from "zustand";
import { emit } from "@tauri-apps/api/event";
import type { PlaybackMedium } from "../models/playback";
import type { ContentSessionState, TranscriptViewState } from "../models/contentSession";
import { log, toErrorFields } from "../repositories";
import { createEventInstaller } from "../utils/eventInstallation";

export const useContentSessionStore = create<ContentSessionState>(() => ({
  textWrap: true,
  textEncodings: {},
  transcriptOpen: { video: false, audio: true },
  transcriptViews: {},
}));

const install = createEventInstaller(
  async (listeners) => {
    await listeners.listen<ContentSessionState>(
      "content-session://state",
      ({ payload }) => {
        useContentSessionStore.setState(payload);
      },
    );
    await emit("content-session://client-ready", {});
  },
  (error) => log.error("content session listener failed", toErrorFields(error)),
  { propagateFailure: true },
);

export function installContentSessionClient(): Promise<void> {
  return install();
}

async function send(event: string, payload: unknown): Promise<void> {
  await installContentSessionClient();
  await emit(event, payload);
}

export function setTextWrap(wrap: boolean): Promise<void> {
  return send("content-session://set-text-wrap", { wrap });
}

export function setTextEncoding(key: string, encoding: string): Promise<void> {
  return send("content-session://set-text-encoding", { key, encoding });
}

export function setTranscriptOpen(medium: PlaybackMedium, open: boolean): Promise<void> {
  return send("content-session://set-transcript-open", { medium, open });
}

export function setTranscriptView(key: string, view: TranscriptViewState): Promise<void> {
  return send("content-session://set-transcript-view", { key, view });
}

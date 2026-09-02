import { create } from "zustand";
import { emit, listen } from "@tauri-apps/api/event";
import type { PlaybackMedium } from "../models/playback";
import type { ContentSessionState, TranscriptViewState } from "../models/contentSession";
import { log, toErrorFields } from "../repositories";

export const useContentSessionStore = create<ContentSessionState>(() => ({
  textWrap: true,
  textEncodings: {},
  transcriptOpen: { video: false, audio: true },
  transcriptViews: {},
}));

let installation: Promise<void> | null = null;

export function installContentSessionClient(): Promise<void> {
  if (installation !== null) return installation;
  installation = listen<ContentSessionState>("content-session://state", ({ payload }) => {
    useContentSessionStore.setState(payload);
  })
    .then(() => emit("content-session://client-ready", {}))
    .then(() => undefined)
    .catch((error) => {
      installation = null;
      log.error("content session listener failed", toErrorFields(error));
      throw error;
    });
  return installation;
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
